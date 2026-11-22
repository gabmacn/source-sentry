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

use std::collections::BTreeMap;
use std::fmt::Write as _;
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
	/// Whether its reread is due, which a plain `y` records when none of its
	/// sources changed.
	rereads: bool,
}

/// What `a` would record: the due documents, and those not due yet, each
/// with its frozen comparison, and how many of each the reviewer has not
/// seen as they are now. Among them, the documents that can only be recorded
/// by a reread, which `a` records only when the reviewer includes them.
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
	/// The documents, due or not, that `a` would record by a reread, and
	/// why.
	rereads: BTreeMap<state::Key, BulkReread>,
	/// Whether the full screen's confirmation includes them, which the
	/// reviewer toggles. The line mode asks instead.
	with_rereads: bool,
}

/// A document that `a` can only record by a reread.
#[derive(Clone, Debug, PartialEq, Eq)]
struct BulkReread {
	/// Its name as the session shows it, relative to the effective
	/// directory.
	name: String,
	/// Why a reread is the review it takes.
	why: RereadWhy,
	/// Whether the reviewer has not seen its comparison as it is now.
	unseen: bool,
}

/// Why `a` would record a document by a reread.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RereadWhy {
	/// Its reread is due and none of its sources changed, so a reread is all
	/// that is left to confirm, as a plain `y` records it.
	Due,
	/// It has no sources, so a reread is the only review it can take.
	NoSources,
}

impl Batch {
	/// The documents that an answer records, each with whether it records a
	/// reread: the due ones, and with `later` those not due yet too, leaving
	/// out the rereads unless `rereads` includes them.
	fn chosen(
		&self,
		later: bool,
		rereads: bool,
	) -> Vec<(state::Key, DocumentDiff, bool)> {
		let listed = if later {
			self.due.iter().chain(&self.later).collect::<Vec<_>>()
		} else {
			self.due.iter().collect()
		};
		listed
			.into_iter()
			.filter_map(|(id, compared)| {
				let reread = self.rereads.contains_key(id);
				(rereads || !reread).then(|| (id.clone(), compared.clone(), reread))
			})
			.collect()
	}

	/// How many of the due documents, or of those not due yet with `later`,
	/// an answer records, with the rereads or without, and how many of them
	/// the reviewer has not seen.
	fn counts(&self, later: bool, rereads: bool) -> (usize, usize) {
		let (listed, unseen) = if later {
			(&self.later, self.unseen_later)
		} else {
			(&self.due, self.unseen)
		};
		if rereads {
			return (listed.len(), unseen);
		}

		let left: Vec<&BulkReread> = listed
			.iter()
			.filter_map(|(id, _)| self.rereads.get(id))
			.collect();
		let left_unseen = left.iter().filter(|reread| reread.unseen).count();
		(listed.len() - left.len(), unseen - left_unseen)
	}

	/// The rereads among the due documents, and with `later` among those
	/// not due yet too: how many there are for each reason, its reread due
	/// or no sources, and how many the reviewer has not seen.
	fn reread_counts(&self, later: bool) -> (usize, usize, usize) {
		let listed = self
			.due
			.iter()
			.chain(self.later.iter().filter(|_| later))
			.filter_map(|(id, _)| self.rereads.get(id));
		let (mut due, mut sourceless, mut unseen) = (0, 0, 0);
		for reread in listed {
			match reread.why {
				RereadWhy::Due => due += 1,
				RereadWhy::NoSources => sourceless += 1,
			}
			if reread.unseen {
				unseen += 1;
			}
		}
		(due, sourceless, unseen)
	}

	/// The rereads of [`Self::reread_counts`], named for people: `1 whose
	/// reread is due and whose sources did not change and 2 without
	/// sources`.
	fn rereads_named(&self, later: bool) -> String {
		let (due, sourceless, _) = self.reread_counts(later);
		let mut parts = Vec::new();
		if due > 0 {
			parts.push(format!(
				"{due} whose reread is due and whose sources did not change"
			));
		}
		if sourceless > 0 {
			parts.push(format!("{sourceless} without sources"));
		}
		parts.join(" and ")
	}

	/// Whether only the question about rereads is left: `Some(later)` when
	/// every document of the batch can only be recorded by a reread and
	/// there is no choice between the due ones and those not due yet,
	/// because one of the two is empty, with `later` when they are not due
	/// yet. `None` when the reviewer chooses the documents first.
	fn rereads_alone(&self) -> Option<bool> {
		let every = self.rereads.len() == self.due.len() + self.later.len();
		match (every, self.due.is_empty(), self.later.is_empty()) {
			(true, false, true) => Some(false),
			(true, true, false) => Some(true),
			_ => None,
		}
	}

	/// The answer when the reviewer declines the rereads among the
	/// documents chosen with `later`: the others, or nothing at all when
	/// every chosen document can only be recorded by a reread.
	fn without_rereads(&self, later: bool) -> Bulk {
		if self.chosen(later, false).is_empty() {
			Bulk::Leave
		} else {
			Bulk::Record {
				later,
				rereads: false,
			}
		}
	}

	/// The rereads among the due documents, and with `later` among those
	/// not due yet too, one line each, in the order they would be recorded:
	/// the document's name, why it can only be recorded by a reread, and
	/// whether the reviewer has not seen it. Empty when there are none.
	fn rereads_listing(&self, later: bool) -> String {
		let mut text = String::new();
		let listed = self
			.due
			.iter()
			.chain(self.later.iter().filter(|_| later))
			.filter_map(|(id, _)| self.rereads.get(id));
		for reread in listed {
			let why = match reread.why {
				RereadWhy::Due => "its reread is due and no source changed",
				RereadWhy::NoSources => "it has no sources",
			};
			let seen = if reread.unseen { ", not seen yet" } else { "" };
			let _ = writeln!(text, "  {}  {why}{seen}", reread.name);
		}
		text
	}
}

/// The answer to the confirmation of `a`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Bulk {
	/// Record nothing, and go on.
	Leave,
	/// Record the due documents, and with `later` those not due yet too,
	/// including those recorded by a reread only with `rereads`.
	Record {
		/// Whether the documents not due yet are recorded too.
		later: bool,
		/// Whether the documents that can only be recorded by a reread are
		/// recorded, each as reread.
		rereads: bool,
	},
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
	/// Confirms the current batch, explicitly naming its unseen count,
	/// offering to include the documents not due yet when it has any, and
	/// asking whether to include the documents that can only be recorded by
	/// a reread, saying that this records that each was reread. `None` ends
	/// the session rather than accepting the batch.
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
