//! Interactive review, with terminal I/O separated from session decisions.

mod external;
mod render;
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
}

/// The session's human interaction boundary. Scripted tests use the same
/// session and write path without requiring terminal handles.
trait Interface {
	/// Shows a document before asking for its answer.
	fn show(&mut self, workspace: &Workspace, view: &View);
	/// Asks for one answer; `None` means end of input or cancellation.
	fn answer(&mut self) -> Result<Option<Answer>, Failure>;
	/// Confirms the current batch, explicitly naming its unseen count.
	/// `None` ends the session rather than accepting the batch.
	fn confirm_all(
		&mut self,
		remaining: usize,
		unseen: usize,
	) -> Result<Option<bool>, Failure>;
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
	/// Reports a refusal, a completed write, or the closing evaluation.
	fn message(&mut self, text: &str);
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

/// Runs an interactive session after the caller verifies its terminal handles.
pub(super) fn run(
	workspace: &Workspace,
	args: &ReviewArgs,
) -> Result<u8, Failure> {
	session::run(workspace, args, &mut terminal::Terminal)?;
	Ok(exit::SUCCESS)
}
