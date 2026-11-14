//! Interactive review, with terminal I/O separated from session decisions.

mod banner;
mod changes;
mod closing;
mod external;
mod opening;
mod render;
mod screen;
mod session;
mod state;
mod terminal;

#[cfg(test)]
mod tests;

use std::io::{self, IsTerminal as _};

use source_sentry_core::model::DocumentId;
use source_sentry_core::mutate::DocumentDiff;

use crate::cli::ReviewArgs;
use crate::exit::{self, Failure};
use crate::output::Printer;
use crate::workspace::Workspace;

/// One document's answer. Absence of an answer ends the session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Answer {
	/// Leave the document as it is until a source changes.
	Next,
	/// Attest exactly the sources shown.
	Yes,
	/// Attest the sources and record a whole-document reread.
	Reread,
	/// Confirm attesting all documents currently remaining.
	All,
	/// Open the complete diff, then ask again.
	Diff,
	/// Edit the document, then evaluate again.
	Edit,
	/// End the session, preserving all earlier writes.
	Quit,
}

impl Answer {
	/// Parses a one-letter answer, with an empty answer meaning `n`.
	fn parse(value: &str) -> Option<Self> {
		match value.trim().to_ascii_lowercase().as_str() {
			"" | "n" => Some(Self::Next),
			"y" => Some(Self::Yes),
			"r" => Some(Self::Reread),
			"a" => Some(Self::All),
			"d" => Some(Self::Diff),
			"e" => Some(Self::Edit),
			"q" => Some(Self::Quit),
			_ => None,
		}
	}
}

/// A displayed comparison and its position in the current queue. The
/// expectations remain unchanged until this document is shown again.
struct View {
	/// The project owning this comparison.
	project: source_sentry_core::model::RepoPath,
	/// The comparison approved by `y` or `r`.
	compared: DocumentDiff,
	/// The document's position, starting at one.
	position: usize,
	/// The positions answered so far plus the current queue length.
	total: usize,
	/// Why the document joined or returned during the session.
	reason: Option<String>,
	/// The facts of the evaluation that queued the document.
	facts: String,
	/// The outcome of those facts in the session's context.
	outcome: source_sentry_core::model::Outcome,
}

/// What `a` would record: the due documents, and those not due yet, each
/// with its frozen comparison, and how many of each the reviewer has not
/// seen as they are now.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Batch {
	/// The documents that are due, or wait on one that is.
	due: Vec<(state::Key, DocumentDiff)>,
	/// The documents that need review but are not due yet.
	later: Vec<(state::Key, DocumentDiff)>,
	/// How many due documents the reviewer has not seen.
	unseen: usize,
	/// How many documents not due yet the reviewer has not seen.
	unseen_later: usize,
}

/// The answer to the confirmation of `a`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Bulk {
	/// Record nothing, and go on.
	Leave,
	/// Record the due documents: the default, which keeps the others for
	/// their review day.
	Due,
	/// Record the documents not due yet as well.
	Everything,
}

/// The session's human interaction boundary. Scripted tests use the same
/// session and write path without requiring terminal handles.
trait Interface {
	/// Presents the session ahead, once, before its first document. A session
	/// whose queue starts empty has no opening.
	fn open(&mut self, opening: &opening::Opening);
	/// Shows a document before asking for its answer.
	fn show(&mut self, workspace: &Workspace, view: &View);
	/// Asks for one answer; `None` means end of input or cancellation.
	fn answer(&mut self) -> Result<Option<Answer>, Failure>;
	/// Confirms the current batch, explicitly naming its unseen count, and
	/// offering to include the documents not due yet when it has any.
	/// `None` ends the session rather than accepting the batch.
	fn confirm_all(&mut self, batch: &Batch) -> Result<Option<Bulk>, Failure>;
	/// Opens the full comparison and waits for the viewer to close.
	fn full_diff(
		&mut self,
		workspace: &Workspace,
		compared: &DocumentDiff,
	) -> Result<(), Failure>;
	/// Opens a local document and waits for the editor to close.
	fn edit(
		&mut self,
		workspace: &Workspace,
		document: &DocumentId,
	) -> Result<(), Failure>;
	/// Reports a refusal or a completed write.
	fn message(&mut self, text: &str);
	/// Presents what was recorded and what remains as the session ends: the
	/// part the session knows at once, then the part that the final
	/// evaluation decides.
	fn close(&mut self, closing: &closing::Closing, part: closing::Part);
}

/// Refuses redirected input or output before even opening the repository.
pub(super) fn require_terminal() -> Result<(), Failure> {
	if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
		return Err(Failure::new(
			exit::NOT_CONFIRMED,
			"review needs terminal input and output; use `diff` to inspect \
			 changes and `attest` to record a review",
		));
	}
	Ok(())
}

/// Runs an interactive session after the caller verifies its terminal handles:
/// on the full screen where the terminal can show it, and otherwise, or with
/// `--plain`, one question after another. `printer` carries `--quiet`.
pub(super) fn run(
	workspace: &Workspace,
	args: &ReviewArgs,
	printer: Printer,
) -> Result<u8, Failure> {
	if !args.plain && screen::supported() {
		screen::run(workspace, args, printer)?;
	} else {
		session::run(workspace, args, &mut terminal::Terminal::new(printer))?;
	}
	Ok(exit::SUCCESS)
}
