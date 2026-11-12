//! Printing a plan, confirming it and applying it.
//!
//! A command that changes files prints its plan first. On a terminal it then
//! asks; `--yes` skips the question, and `--dry-run` prints the plan only.
//! Without a terminal and without `--yes`, it refuses, so CI never changes
//! files silently.

use std::fmt::Write as _;
use std::io::{self, BufRead as _, IsTerminal as _, Write as _};

use source_sentry_core::ports::WritePlan;

use crate::cli::ConfirmArgs;
use crate::exit::{self, Failure};
use crate::output::{self, Printer};
use crate::workspace::Workspace;

/// Prints `summary` and the files `plan` touches, confirms, and applies the
/// plan through the backend, all or nothing.
///
/// Returns 0 when the plan was applied, was empty, or `--dry-run` was
/// given, and 1 when the user declined. Under `--quiet`, the plan is printed
/// only when a question needs it.
///
/// # Errors
///
/// Exit 1 without a terminal and without `--yes`, and 4 when the plan
/// cannot be applied or the answer cannot be read.
pub(super) fn confirm_and_apply(
	workspace: &Workspace,
	plan: &WritePlan,
	summary: &str,
	confirm: ConfirmArgs,
	printer: Printer,
) -> Result<u8, Failure> {
	let mut text = summary.trim_end().to_owned();
	if plan.writes.is_empty() && plan.deletes.is_empty() {
		text.push_str("\n\nNothing to change.");
		printer.print(&text);
		return Ok(exit::SUCCESS);
	}
	text.push_str("\n\n");
	text.push_str(&files(workspace, plan));
	if confirm.dry_run {
		text.push_str("\n\nDry run: nothing was written.");
		printer.print(&text);
		return Ok(exit::SUCCESS);
	}
	if confirm.yes {
		printer.print(&text);
	} else {
		if !io::stdin().is_terminal() {
			printer.print(&text);
			return Err(Failure::new(
				exit::NOT_CONFIRMED,
				"no terminal to confirm on, so nothing was changed; pass --yes to \
				 apply, or --dry-run to preview",
			));
		}
		// The question is never asked blind, so the plan is shown even under
		// `--quiet`, on standard error with the question.
		if printer.is_quiet() {
			output::write_err(&text);
		} else {
			printer.print(&text);
		}
		if !ask("Apply these changes? [y/N] ")? {
			printer.note("Nothing was changed.");
			return Ok(exit::NOT_CONFIRMED);
		}
	}
	workspace
		.repository
		.apply(plan)
		.map_err(|error| Failure::apply(workspace, &error))?;
	printer.print(&applied(workspace, plan));
	Ok(exit::SUCCESS)
}

/// The files a plan writes, creates and deletes.
fn files(workspace: &Workspace, plan: &WritePlan) -> String {
	let mut lines = Vec::new();
	for write in &plan.writes {
		let verb = if write.expected.is_some() {
			"write "
		} else {
			"create"
		};
		lines.push(format!("  {verb}  {}", workspace.display_path(&write.path)));
	}
	for delete in &plan.deletes {
		lines.push(format!(
			"  delete  {}",
			workspace.display_path(&delete.path)
		));
	}
	let mut text = format!("Files:\n{}", lines.join("\n"));
	if plan.writes.iter().any(|write| write.expected.is_none()) {
		// The plan says so before it is applied.
		text.push_str(
			"\nCreated files will be marked intent-to-add: git records their \
			 paths, and you stage their contents as usual.",
		);
	}
	text
}

/// What was done. Created files are marked intent-to-add, so the next
/// snapshot sees them, but their contents are not staged.
fn applied(workspace: &Workspace, plan: &WritePlan) -> String {
	let count = plan.writes.len() + plan.deletes.len();
	let mut text = format!(
		"Applied: {} changed.",
		crate::describe::count(count, "file", "files")
	);
	// With `-C`, the command names the repository and its paths, so that
	// it works from wherever it is run; otherwise paths are relative to the
	// current directory, as git expects.
	let repo = workspace.repo_argument();
	let created: Vec<String> = plan
		.writes
		.iter()
		.filter(|write| write.expected.is_none())
		.map(|write| {
			let path = workspace.display_path(&write.path);
			super::diff::shell_word(&path)
		})
		.collect();
	if !created.is_empty() {
		let git = repo.map_or_else(
			|| "git".to_owned(),
			|repo| {
				format!(
					"git -C {}",
					super::diff::shell_word(&repo.display().to_string())
				)
			},
		);
		let _ = write!(
			text,
			"\nThe new files are marked intent-to-add. Stage them with the rest \
			 of your change:\n  {git} add -- {}",
			created.join(" ")
		);
	}
	text
}

/// Asks `question` on standard error and reads the answer from standard
/// input: `y` or `yes`, in any case, confirms.
fn ask(question: &str) -> Result<bool, Failure> {
	let mut stderr = anstream::stderr();
	let _ = write!(stderr, "{question}");
	let _ = stderr.flush();
	let mut answer = String::new();
	io::stdin().lock().read_line(&mut answer).map_err(|error| {
		Failure::new(
			exit::OPERATIONAL,
			format!("cannot read the answer: {error}"),
		)
	})?;
	let answer = answer.trim();
	Ok(answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes"))
}
