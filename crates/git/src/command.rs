//! Runs the `git` executable against the repository that `git2` opened.

use std::{
	io::{self, Write as _},
	path::{Path, PathBuf},
	process::{Command, ExitStatus, Stdio},
	thread,
};

use source_sentry_core::ports::{BackendError, BackendErrorKind};

use crate::error::{caused, failure};

/// Environment variables that would point git at another index or repository
/// layout than the one `git2` opened. They are removed from every child.
const REDIRECTING_VARIABLES: [&str; 4] = [
	"GIT_INDEX_FILE",
	"GIT_COMMON_DIR",
	"GIT_OBJECT_DIRECTORY",
	"GIT_ALTERNATE_OBJECT_DIRECTORIES",
];

/// Configuration that turns the Git LFS filter driver into a no-op for one
/// command, so that git applies only its built-in conversions to a path with
/// `filter=lfs`. Git skips a driver whose commands are empty, and `required`
/// would otherwise make it fail instead (the LFS clean filter never
/// runs).
pub(crate) const LFS_DRIVER_DISABLED: [&str; 8] = [
	"-c",
	"filter.lfs.clean=",
	"-c",
	"filter.lfs.smudge=",
	"-c",
	"filter.lfs.process=",
	"-c",
	"filter.lfs.required=false",
];

/// What a finished `git` command printed.
pub(crate) struct Finished {
	/// The exit status.
	pub status: ExitStatus,
	/// Standard output.
	pub stdout: Vec<u8>,
	/// Standard error.
	pub stderr: Vec<u8>,
}

/// Builds `git` commands bound to one repository.
///
/// Every command gets `GIT_DIR` and, for a non-bare repository,
/// `GIT_WORK_TREE`, so that it reads exactly the repository `git2` opened. It
/// also sets `GIT_NO_LAZY_FETCH`, because a partial clone must never fetch
/// during `check`, and `GIT_OPTIONAL_LOCKS=0`, so that no command
/// refreshes the index.
#[derive(Clone, Debug)]
pub(crate) struct Git {
	git_dir: PathBuf,
	work_dir: Option<PathBuf>,
}

impl Git {
	/// Binds commands to the repository at `git_dir`, with the working tree
	/// at `work_dir` when it has one.
	pub(crate) fn new(git_dir: &Path, work_dir: Option<&Path>) -> Self {
		Self {
			git_dir: git_dir.to_path_buf(),
			work_dir: work_dir.map(Path::to_path_buf),
		}
	}

	/// Runs `git` with `args`, feeding `input` to its standard input, and
	/// returns its output whatever its exit status.
	///
	/// # Errors
	///
	/// Fails with [`BackendErrorKind::GitExecutable`] when `git` cannot be
	/// started, for example because it is not on the `PATH`, or when its
	/// standard input or output breaks.
	pub(crate) fn run(
		&self,
		args: &[&str],
		input: Option<&[u8]>,
	) -> Result<Finished, BackendError> {
		let mut command = Command::new("git");
		command
			.args(args)
			.env("GIT_DIR", &self.git_dir)
			.env("GIT_NO_LAZY_FETCH", "1")
			.env("GIT_OPTIONAL_LOCKS", "0")
			.env("GIT_TERMINAL_PROMPT", "0")
			.stdin(if input.is_some() {
				Stdio::piped()
			} else {
				Stdio::null()
			})
			.stdout(Stdio::piped())
			.stderr(Stdio::piped());
		for variable in REDIRECTING_VARIABLES {
			command.env_remove(variable);
		}
		match &self.work_dir {
			Some(work_dir) => {
				command.env("GIT_WORK_TREE", work_dir).current_dir(work_dir);
			}
			None => {
				command
					.env_remove("GIT_WORK_TREE")
					.current_dir(&self.git_dir);
			}
		}

		let mut child = command
			.spawn()
			.map_err(|error| spawn_failure(args, error))?;
		let stdin = child.stdin.take();
		// The input is written from a second thread while this one drains the
		// output, because git answers `--stdin-paths` line by line and would
		// block on a full output pipe while this side blocks on a full input
		// pipe.
		let (output, written) = thread::scope(|scope| {
			let writer = scope.spawn(move || -> io::Result<()> {
				if let (Some(mut pipe), Some(bytes)) = (stdin, input) {
					pipe.write_all(bytes)?;
				}
				Ok(())
			});
			let output = child.wait_with_output();
			let written = writer
				.join()
				.unwrap_or_else(|_| Err(io::Error::other("the input writer panicked")));
			(output, written)
		});
		let output = output.map_err(|error| {
			caused(
				BackendErrorKind::GitExecutable,
				format!("lost contact with `git {}`", args.join(" ")),
				error,
			)
		})?;
		// A failed command often stops reading early; its exit status and
		// message explain more than the broken pipe does.
		if output.status.success() {
			written.map_err(|error| {
				caused(
					BackendErrorKind::GitExecutable,
					format!("cannot send input to `git {}`", args.join(" ")),
					error,
				)
			})?;
		}
		Ok(Finished {
			status: output.status,
			stdout: output.stdout,
			stderr: output.stderr,
		})
	}

	/// Runs `git` like [`Git::run`] and returns its standard output.
	///
	/// # Errors
	///
	/// Fails like [`Git::run`], and when `git` exits unsuccessfully.
	pub(crate) fn output(
		&self,
		args: &[&str],
		input: Option<&[u8]>,
	) -> Result<Vec<u8>, BackendError> {
		let finished = self.run(args, input)?;
		if finished.status.success() {
			Ok(finished.stdout)
		} else {
			Err(exit_failure(args, &finished))
		}
	}
}

/// Describes a `git` command that exited unsuccessfully.
pub(crate) fn exit_failure(args: &[&str], finished: &Finished) -> BackendError {
	let message = String::from_utf8_lossy(&finished.stderr);
	failure(
		BackendErrorKind::GitExecutable,
		format!(
			"`git {}` failed ({}): {}{}",
			args.join(" "),
			finished.status,
			message.trim(),
			hint(&message)
		),
	)
}

/// Advice for a failure whose remedy is a git setting.
fn hint(message: &str) -> &'static str {
	if message.contains("Filename too long") {
		". On Windows, `git config core.longpaths true` lets git read paths \
		 longer than 260 characters"
	} else {
		""
	}
}

/// Describes a `git` command that could not be started.
fn spawn_failure(args: &[&str], error: io::Error) -> BackendError {
	let message = if error.kind() == io::ErrorKind::NotFound {
		"the `git` executable was not found on the PATH; Source Sentry needs it \
		 to hash the working tree"
			.to_owned()
	} else {
		format!("cannot start `git {}`", args.join(" "))
	};
	caused(BackendErrorKind::GitExecutable, message, error)
}
