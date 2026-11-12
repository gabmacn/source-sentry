//! Current-directory paths, using child processes to isolate their directories.

use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;

use serde_json::{Value, json};

use super::super::{Answer, session, state::State};
use super::support::{Repo, Script, Step, local};
use crate::cli::{DocumentArg, ReviewArgs};
use crate::workspace::Workspace;

/// Shows located changes and closing paths from each invocation directory,
/// preserving an explicit repository in the command printed for copying.
#[test]
fn paths_and_the_closing_command_use_the_current_directory() {
	if let Ok(case) = env::var("SOURCE_SENTRY_REVIEW_PATH_CASE") {
		let case: Value = serde_json::from_str(&case).unwrap();
		check_paths(&case);
		return;
	}
	for (directory, repository, document, source) in [
		(".", None, "docs/a.md", "src/a.rs"),
		("docs", None, "a.md", "../src/a.rs"),
		("src", Some(".."), "docs/a.md", "src/a.rs"),
		("docs", Some("../."), "docs/a.md", "src/a.rs"),
		("..", Some(""), "", ""),
	] {
		let repo = Repo::new(json!({
			"docs/a.md": {"sources": [{"path": "/src/a.rs"}]},
			"docs/b.md": {}
		}));
		let name = repo.directory.path().file_name().unwrap().to_str().unwrap();
		let outside = directory == "..";
		let case = json!({
			"root": repo.directory.path(),
			"repo": if outside { Some(name) } else { repository },
			"document": if outside {
				"docs/a.md".to_owned()
			} else {
				document.to_owned()
			},
			"source": if outside {
				"src/a.rs".to_owned()
			} else {
				source.to_owned()
			}
		});
		let output = Command::new(env::current_exe().unwrap())
			.args([
				"--exact",
				"commands::review::tests::paths::paths_and_the_closing_command_use_the_current_directory",
				"--nocapture",
			])
			.current_dir(repo.directory.path().join(directory))
			.env("SOURCE_SENTRY_REVIEW_PATH_CASE", case.to_string())
			.output()
			.unwrap();
		assert!(
			output.status.success(),
			"{case}:\n{}\n{}",
			String::from_utf8_lossy(&output.stdout),
			String::from_utf8_lossy(&output.stderr)
		);
	}
}

/// Exercises the real session without changing the test process's directory.
fn check_paths(case: &Value) {
	let root = Path::new(case["root"].as_str().unwrap());
	let repository = case["repo"].as_str().map(Path::new);
	let workspace = Workspace::open(repository).unwrap();
	let document = case["document"].as_str().unwrap();
	let source = case["source"].as_str().unwrap();
	let args = ReviewArgs {
		documents: vec![DocumentArg::Path(document.into())],
		context: "default".to_owned(),
		reviewer: Some("Test Reviewer".to_owned()),
	};
	let mut script = Script::at(
		root,
		[Answer::Diff, Answer::Yes]
			.into_iter()
			.map(|answer| Step::Answer(Some(answer))),
	);
	session::run(&workspace, &args, &mut script).unwrap();
	assert_eq!(script.shown, [local("docs/a.md")]);
	assert!(script.displays[0].contains(&format!("[1/1] {document}:")));
	for text in [&script.displays[0], &script.diffs[0]] {
		assert!(text.contains(&format!("\n{source}:1  unattested ->")));
		assert!(text.contains(&format!("\n{source}:1  +old a")));
	}
	assert!(script.summary().contains(&format!("\n  {document}\n")));
	let remaining = format!("  {}: ", document.replace("a.md", "b.md"));
	let remaining = script
		.summary()
		.lines()
		.find(|line| line.starts_with(&remaining))
		.unwrap();
	assert!(remaining.contains("unattested"));
	assert!(remaining.ends_with("; outside the selected documents"));
	let prefix = repository.map_or_else(
		|| "source-sentry".to_owned(),
		|path| format!("source-sentry -C {}", path.display()),
	);
	assert!(
		script
			.summary()
			.ends_with(&format!("Run `{prefix} check` for the final gate."))
	);
	fs::write(
		root.join(".source-sentry/source-sentry.yaml"),
		"version: 2\n",
	)
	.unwrap();
	let error = State::load(&workspace, "default").err().unwrap();
	assert_eq!(error.code, 3);
	assert!(error.message.contains(&format!(
		"{}:1",
		workspace.display_path(
			&source_sentry_core::model::RepoPath::new(
				".source-sentry/source-sentry.yaml"
			)
			.unwrap()
		)
	)));
}
