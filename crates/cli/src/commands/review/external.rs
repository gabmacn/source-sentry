//! Editor and pager selection, and synchronous shell-command launching.

use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::Path;
use std::process::Command;

use source_sentry_core::model::DocumentId;
use source_sentry_core::mutate::DocumentDiff;

use crate::exit::{self, Failure};
use crate::output;
use crate::workspace::Workspace;

/// Opens the complete diff with the configured editor, then pager, falling
/// back to plain output when neither is configured.
pub(super) fn full_diff(
	workspace: &Workspace,
	compared: &DocumentDiff,
) -> Result<(), Failure> {
	let text = super::render::changes(workspace, compared, None);
	let viewer = match editor(workspace)? {
		Some(editor) => Some(editor),
		None => pager(workspace)?,
	};
	let Some(viewer) = viewer else {
		output::data(&text);
		return Ok(());
	};
	let file = tempfile::Builder::new()
		.prefix("source-sentry-review-")
		.suffix(".diff")
		.tempfile()
		.map_err(|error| failed("cannot create a temporary diff", &error))?;
	// Close the handle before an editor opens it, including on Windows.
	let path = file.into_temp_path();
	fs::write(&path, text)
		.map_err(|error| failed("cannot write the temporary diff", &error))?;
	launch(&viewer, &path)
}

/// Edits a local document, without allowing a pager to stand in for an editor.
pub(super) fn edit(
	workspace: &Workspace,
	document: &DocumentId,
) -> Result<(), Failure> {
	let DocumentId::Local(path) = document else {
		return Err(Failure::new(
			exit::REFUSED,
			"a remote document cannot be opened in a local editor; open it in its source application",
		));
	};
	let editor = editor(workspace)?.ok_or_else(|| {
		Failure::new(
			exit::REFUSED,
			"no editor is configured; set VISUAL, EDITOR or git's core.editor",
		)
	})?;
	let path = workspace.display_path(path);
	// Prefix relative paths so a file beginning with `-` is never an option.
	let path = Path::new(&path);
	let target = if path.is_absolute() {
		path.to_path_buf()
	} else {
		Path::new(".").join(path)
	};
	launch(&editor, &target)
}

/// The editor chain: VISUAL, EDITOR, then git's effective core.editor.
fn editor(workspace: &Workspace) -> Result<Option<String>, Failure> {
	configured(workspace, &["VISUAL", "EDITOR"], "core.editor", |name| {
		env::var_os(name)
	})
}

/// The pager chain: `GIT_PAGER`, PAGER, then git's effective core.pager.
fn pager(workspace: &Workspace) -> Result<Option<String>, Failure> {
	configured(workspace, &["GIT_PAGER", "PAGER"], "core.pager", |name| {
		env::var_os(name)
	})
}

/// Finds the first nonempty configured command. Git reads its own config so
/// includes and all configuration levels have their usual meaning.
fn configured(
	workspace: &Workspace,
	variables: &[&str],
	key: &str,
	environment: impl Fn(&str) -> Option<OsString>,
) -> Result<Option<String>, Failure> {
	for variable in variables {
		if let Some(value) = environment(variable) {
			let value = value.into_string().map_err(|_| {
				Failure::new(
					exit::OPERATIONAL,
					format!("{variable} is not valid UTF-8"),
				)
			})?;
			if !value.trim().is_empty() {
				return Ok(Some(value));
			}
		}
	}
	let directory = workspace.repository.work_dir().ok_or_else(|| {
		Failure::new(exit::OPERATIONAL, "review needs a working tree")
	})?;
	let result = Command::new("git")
		.arg("-C")
		.arg(directory)
		.args(["config", "--get", key])
		.output()
		.map_err(|error| {
			failed("cannot read git's viewer configuration", &error)
		})?;
	if result.status.code() == Some(1) {
		return Ok(None);
	}
	if !result.status.success() {
		return Err(Failure::new(
			exit::OPERATIONAL,
			format!(
				"cannot read git's {key}: {}",
				String::from_utf8_lossy(&result.stderr).trim()
			),
		));
	}
	let value = String::from_utf8(result.stdout).map_err(|error| {
		failed("git's viewer configuration is not valid UTF-8", &error)
	})?;
	Ok((!value.trim().is_empty()).then(|| value.trim().to_owned()))
}

/// Runs an editor or pager shell command with the file as a separate argument
/// and waits for it to close. Unix uses the shell's positional argument;
/// Windows expands an environment variable with delayed expansion disabled.
fn launch(command: &str, path: &Path) -> Result<(), Failure> {
	let mut process = shell(command, path.as_os_str());
	let status = process
		.status()
		.map_err(|error| failed("cannot start the editor or pager", &error))?;
	if status.success() {
		Ok(())
	} else {
		Err(Failure::new(
			exit::OPERATIONAL,
			format!("the editor or pager exited with {status}"),
		))
	}
}

/// Builds the platform shell invocation without interpolating the file path.
#[cfg(windows)]
fn shell(command: &str, path: &OsStr) -> Command {
	use std::os::windows::process::CommandExt as _;

	let mut process = Command::new("cmd");
	process
		.args(["/D", "/V:OFF", "/S", "/C"])
		.raw_arg(format!("\"{command} \"%SOURCE_SENTRY_REVIEW_FILE%\"\""))
		.env("SOURCE_SENTRY_REVIEW_FILE", path);
	process
}

/// Builds the platform shell invocation without interpolating the file path.
#[cfg(not(windows))]
fn shell(command: &str, path: &OsStr) -> Command {
	let mut process = Command::new("sh");
	process
		.args(["-c", &format!("{command} \"$1\""), "source-sentry-review"])
		.arg(path);
	process
}

/// An editor or pager I/O error, retaining its context.
fn failed(context: &str, error: &impl std::fmt::Display) -> Failure {
	Failure::new(exit::OPERATIONAL, format!("{context}: {error}"))
}

#[cfg(test)]
mod tests {
	use std::ffi::OsString;
	use std::fs;

	use serde_json::json;

	use super::{configured, launch};
	use crate::commands::review::tests::support::Repo;

	/// Environment commands precede git configuration, in the specified order.
	#[test]
	fn editor_and_pager_priorities_use_the_first_nonempty_command() {
		let repo = Repo::new(json!({}));
		repo.git(&["config", "core.editor", "configured-editor --wait"]);
		repo.git(&["config", "core.pager", "configured-pager"]);
		for (variables, key) in [
			(["VISUAL", "EDITOR"], "core.editor"),
			(["GIT_PAGER", "PAGER"], "core.pager"),
		] {
			let workspace = repo.workspace();
			let both = configured(&workspace, &variables, key, |name| {
				Some(OsString::from(if name == variables[0] {
					"first --wait"
				} else {
					"second"
				}))
			})
			.unwrap();
			assert_eq!(both.as_deref(), Some("first --wait"));
			let second = configured(&workspace, &variables, key, |name| {
				Some(OsString::from(if name == variables[0] {
					" "
				} else {
					"second"
				}))
			})
			.unwrap();
			assert_eq!(second.as_deref(), Some("second"));
			let git = configured(&workspace, &variables, key, |_| None).unwrap();
			assert_eq!(
				git.as_deref(),
				Some(if key == "core.editor" {
					"configured-editor --wait"
				} else {
					"configured-pager"
				})
			);
		}
	}

	/// A real editor script receives its arguments and the complete file path,
	/// edits the document, and has closed before control returns.
	#[test]
	fn editor_commands_wait_and_preserve_paths_with_spaces_and_shell_characters()
	{
		let directory = tempfile::tempdir().unwrap();
		let target = directory.path().join("a & b %PATH% ! file.md");
		fs::write(&target, "old\n").unwrap();
		#[cfg(windows)]
		let command = {
			let script = directory.path().join("editor with spaces.cmd");
			fs::write(&script, "@echo off\r\nif not \"%~1\"==\"--wait\" exit /b 9\r\n>\"%~2\" echo edited\r\n").unwrap();
			format!("\"{}\" --wait", script.display())
		};
		#[cfg(not(windows))]
		let command = {
			let script = directory.path().join("editor with spaces.sh");
			fs::write(
				&script,
				"[ \"$1\" = --wait ] || exit 9\nprintf 'edited\\n' > \"$2\"\n",
			)
			.unwrap();
			format!(
				"sh {} --wait",
				crate::commands::shell_word(&script.to_string_lossy())
			)
		};
		launch(&command, &target).unwrap();
		assert_eq!(fs::read_to_string(&target).unwrap().trim(), "edited");
	}

	/// A failed editor reports the process status instead of recording a review.
	#[test]
	fn a_failed_editor_retains_the_operational_error() {
		let file = tempfile::NamedTempFile::new().unwrap();
		let error = launch(
			if cfg!(windows) {
				"cmd /C exit 7"
			} else {
				"sh -c 'exit 7'"
			},
			file.path(),
		)
		.unwrap_err();
		assert_eq!(error.code, 4);
		assert!(error.message.contains("exited"));
	}
}
