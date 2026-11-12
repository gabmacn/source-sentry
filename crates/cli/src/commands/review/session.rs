//! The review loop. Every accepted answer plans and applies one attestation
//! immediately; queue decisions always follow a fresh evaluation.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use source_sentry_core::model::{DocumentId, RepoPath};
use source_sentry_core::mutate::{
	AttestRequest, DocumentDiff, Expectation, MutateError, Selection, attest,
	diff,
};

use super::state::{Checkpoint, Stamp, State, changed_sources};
use super::{Answer, Interface, View};
use crate::cli::ReviewArgs;
use crate::exit::{self, Failure};
use crate::workspace::{Workspace, load};

/// Mutable session history; the working tree remains the source of truth.
struct Session<'a> {
	/// The repository and path-display rules.
	workspace: &'a Workspace,
	/// The context used for every evaluation.
	context: &'a str,
	/// Explicitly named documents, or an empty set for the whole repository.
	selected: BTreeSet<DocumentId>,
	/// The reviewer resolved once, as for `attest`.
	reviewer: Option<String>,
	/// The latest completed evaluation.
	state: State,
	/// Documents needing review when the session began.
	initial: BTreeSet<DocumentId>,
	/// Answers that suppress unchanged repeat questions.
	answered: BTreeMap<DocumentId, Checkpoint>,
	/// The source versions whose comparisons the user has actually seen.
	seen: BTreeMap<DocumentId, Vec<Expectation>>,
	/// Applied reviews, including whether each recorded a reread.
	recorded: Vec<(DocumentId, bool)>,
	/// The explanation of changes observed since the session began.
	reasons: BTreeMap<DocumentId, String>,
	/// Answered positions; retries and editor visits keep their position.
	finished: usize,
}

impl<'a> Session<'a> {
	/// Loads and validates the starting evaluation and document selection.
	fn new(
		workspace: &'a Workspace,
		args: &'a ReviewArgs,
	) -> Result<Self, Failure> {
		let state = State::load(workspace, &args.context)?;
		let selected: BTreeSet<_> = args
			.documents
			.iter()
			.map(|argument| workspace.document(argument))
			.collect::<Result<_, _>>()?;
		for id in &selected {
			if !state.tracked.contains(id) {
				return Err(Failure::new(
					exit::REFUSED,
					format!("`{}` is not tracked", workspace.display_document(id)),
				));
			}
		}
		let initial = state
			.documents
			.iter()
			.filter(|(_, document)| document.pending)
			.map(|(id, _)| id.clone())
			.collect();
		Ok(Self {
			workspace,
			context: &args.context,
			selected,
			reviewer: super::super::person(workspace, args.reviewer.as_deref())?,
			state,
			initial,
			answered: BTreeMap::new(),
			seen: BTreeMap::new(),
			recorded: Vec::new(),
			reasons: BTreeMap::new(),
			finished: 0,
		})
	}

	/// Rebuilds the evidence after an answer or edit. Known attestation writes
	/// cannot repeatedly requeue documents whose own review writes a source.
	fn refresh(&mut self, written: &BTreeSet<RepoPath>) -> Result<(), Failure> {
		let next = State::load(self.workspace, self.context)?;
		for (id, document) in &next.documents {
			if let Some(previous) = self.state.documents.get(id) {
				let changed =
					changed_sources(&previous.stamp.sources, &document.stamp.sources);
				if !changed.is_empty() {
					let paths: Vec<_> = changed
						.iter()
						.map(|path| self.workspace.display_path(path))
						.collect();
					self.reasons.insert(
						id.clone(),
						format!(
							"Sources changed during this session: {}.",
							paths.join(", ")
						),
					);
				} else if !previous.pending && document.pending {
					self.reasons.insert(
						id.clone(),
						format!(
							"Started needing review during this session: {}.",
							document.facts
						),
					);
				} else if document.pending
					&& previous.stamp.upstreams != document.stamp.upstreams
				{
					self.reasons.insert(
						id.clone(),
						format!(
							"Upstream review state changed during this session: {}.",
							document.facts
						),
					);
				}
			} else if document.pending {
				self
					.reasons
					.insert(id.clone(), "Newly tracked during this session.".to_owned());
			}
			if let Some(answer) = self.answered.get_mut(id) {
				answer.refresh(document, written);
			}
		}
		self.answered.retain(|id, answer| {
			matches!(answer, Checkpoint::Skipped(_))
				|| next
					.documents
					.get(id)
					.is_some_and(|document| document.pending)
		});
		self.state = next;
		Ok(())
	}

	/// The remaining eligible queue in the current evaluation.
	fn queue(&self) -> Result<Vec<DocumentId>, Failure> {
		self.state.queue(&self.selected, &self.answered)
	}

	/// Captures exactly the comparison whose expectations an answer approves.
	fn compare(&self, id: &DocumentId) -> Result<DocumentDiff, Failure> {
		let snapshot = self
			.workspace
			.repository
			.working_tree_snapshot()
			.map_err(|error| Failure::backend(&error))?;
		let (_, inventory) = load(self.workspace, &snapshot)?;
		diff(
			&snapshot,
			&self.workspace.repository,
			&inventory,
			id,
			&Selection::All,
		)
		.map_err(|error| Failure::mutate(self.workspace, &error))
	}

	/// Records one approved comparison. A changed source refuses this answer
	/// and requests another prompt with a new comparison.
	fn record(
		&mut self,
		compared: &DocumentDiff,
		reread: bool,
		ui: &mut impl Interface,
	) -> Result<bool, Failure> {
		let reread = reread || compared.sources.is_empty();
		let snapshot = self
			.workspace
			.repository
			.working_tree_snapshot()
			.map_err(|error| Failure::backend(&error))?;
		let (_, inventory) = load(self.workspace, &snapshot)?;
		let planned = match attest(
			&snapshot,
			&inventory,
			&AttestRequest {
				document: compared.document.clone(),
				selection: Selection::All,
				expectations: Some(compared.expectations.clone()),
				reviewer: self.reviewer.clone(),
				at: super::super::now(),
				reread,
			},
		) {
			Ok(planned) => planned,
			Err(MutateError::ExpectationMismatch { .. }) => {
				ui.message("A source changed after it was shown; this review was not recorded. Showing the current changes again.");
				self.refresh(&BTreeSet::new())?;
				return Ok(false);
			}
			Err(error) => return Err(Failure::mutate(self.workspace, &error)),
		};
		self
			.workspace
			.repository
			.apply(&planned.plan)
			.map_err(|error| Failure::apply(self.workspace, &error))?;
		let written: BTreeSet<_> = planned
			.plan
			.writes
			.iter()
			.map(|write| write.path.clone())
			.chain(
				planned
					.plan
					.deletes
					.iter()
					.map(|delete| delete.path.clone()),
			)
			.collect();
		self.recorded.push((compared.document.clone(), reread));
		self.finished += 1;
		ui.message(&format!(
			"Recorded {}{}.",
			self.workspace.display_document(&compared.document),
			if reread { " (reread)" } else { "" }
		));
		let before = shown_stamp(compared);
		self.refresh(&written)?;
		if let Some(document) = self.state.documents.get(&compared.document)
			&& document.pending
		{
			let changed = changed_sources(&before.sources, &document.stamp.sources);
			let answer = if changed.iter().any(|path| written.contains(path)) {
				Checkpoint::SelfWritten(document.stamp.clone())
			} else {
				Checkpoint::Reviewed(document.stamp.clone())
			};
			self.answered.insert(compared.document.clone(), answer);
		}
		Ok(true)
	}

	/// Captures and confirms the current queue once. Every source expectation
	/// is frozen before confirmation; cascades outside this batch are prompted
	/// individually, and a changed comparison stops the batch for reinspection.
	fn all(
		&mut self,
		view: &View,
		queue: &[DocumentId],
		ui: &mut impl Interface,
	) -> Result<bool, Failure> {
		let comparisons: Vec<_> = queue
			.iter()
			.map(|id| {
				if id == &view.compared.document {
					Ok(view.compared.clone())
				} else {
					self.compare(id)
				}
			})
			.collect::<Result<_, _>>()?;
		let unseen = comparisons
			.iter()
			.filter(|compared| {
				self.seen.get(&compared.document) != Some(&compared.expectations)
			})
			.count();
		match ui.confirm_all(queue.len(), unseen)? {
			None => return Ok(false),
			Some(false) => return Ok(true),
			Some(true) => {}
		}
		for compared in comparisons {
			if self.queue()?.contains(&compared.document)
				&& !self.record(&compared, false, ui)?
			{
				break;
			}
		}
		Ok(true)
	}

	/// Reports applied reviews and all remaining obligations from the final
	/// evaluation, including documents outside an explicit selection.
	fn summary(&self) -> String {
		let mut text = format!(
			"Recorded {}.\n",
			crate::describe::count(self.recorded.len(), "review", "reviews")
		);
		for (id, reread) in &self.recorded {
			let _ = writeln!(
				text,
				"  {}{}",
				self.workspace.display_document(id),
				if *reread { " (reread)" } else { "" }
			);
		}
		let remaining: Vec<_> = self
			.state
			.documents
			.iter()
			.filter(|(_, document)| document.pending)
			.collect();
		let _ = writeln!(
			text,
			"{} still {} review:",
			crate::describe::count(remaining.len(), "document", "documents"),
			if remaining.len() == 1 {
				"needs"
			} else {
				"need"
			}
		);
		for (id, document) in remaining {
			let _ = write!(
				text,
				"  {}: {}",
				self.workspace.display_document(id),
				document.facts
			);
			if !self.initial.contains(id) {
				text.push_str("; started needing review during this session");
			}
			if !self.selected.is_empty() && !self.selected.contains(id) {
				text.push_str("; outside the selected documents");
			}
			if matches!(self.answered.get(id), Some(Checkpoint::SelfWritten(_))) {
				text.push_str(
					"; recording the review changed a source, so it was not queued again",
				);
			}
			text.push('\n');
		}
		let _ = write!(
			text,
			"Run `{} check` for the final gate.",
			self.workspace.command_prefix()
		);
		text
	}
}

/// Runs the session through `ui`. A final evaluation precedes both an empty
/// queue's completion and a quit summary; answers never stand in for freshness.
pub(super) fn run(
	workspace: &Workspace,
	args: &ReviewArgs,
	ui: &mut impl Interface,
) -> Result<(), Failure> {
	let mut session = Session::new(workspace, args)?;
	let mut retry = None;
	'session: loop {
		let mut queue = session.queue()?;
		if queue.is_empty() {
			session.refresh(&BTreeSet::new())?;
			queue = session.queue()?;
		}
		let Some(id) = retry
			.take()
			.filter(|id| queue.contains(id))
			.or_else(|| queue.first().cloned())
		else {
			break;
		};
		let view = View {
			compared: session.compare(&id)?,
			position: session.finished + 1,
			total: session.finished + queue.len(),
			reason: session.reasons.get(&id).cloned(),
			facts: session.state.documents[&id].facts.clone(),
		};
		session
			.seen
			.insert(id.clone(), view.compared.expectations.clone());
		ui.show(workspace, &view);
		loop {
			match ui.answer()? {
				None | Some(Answer::Quit) => break 'session,
				Some(Answer::Diff) => {
					if let Err(error) = ui.full_diff(workspace, &view.compared) {
						ui.message(&error.message);
					}
				}
				Some(Answer::Edit) => {
					if let Err(error) = ui.edit(workspace, &id) {
						ui.message(&error.message);
					}
					session.refresh(&BTreeSet::new())?;
					retry = Some(id.clone());
					break;
				}
				Some(Answer::Next) => {
					session.finished += 1;
					session.refresh(&BTreeSet::new())?;
					// `n` leaves the state at the answer, including any edit made
					// while its prompt was open. Only a later source change retries.
					if let Some(document) = session.state.documents.get(&id) {
						session
							.answered
							.insert(id.clone(), Checkpoint::Skipped(document.stamp.clone()));
					}
					break;
				}
				Some(answer @ (Answer::Yes | Answer::Reread)) => {
					// These variants share the expectation-bound write path.
					let reread = answer == Answer::Reread;
					if !session.record(&view.compared, reread, ui)? {
						retry = Some(id.clone());
					}
					break;
				}
				Some(Answer::All) => {
					if !session.all(&view, &queue, ui)? {
						break 'session;
					}
					break;
				}
			}
		}
	}
	session.refresh(&BTreeSet::new())?;
	ui.message(&session.summary());
	Ok(())
}

/// A comparison's source states, independent of its baseline attestations.
fn shown_stamp(compared: &DocumentDiff) -> Stamp {
	let sources = compared
		.sources
		.iter()
		.flat_map(|source| &source.entries)
		.map(|entry| {
			let oid = match &entry.current {
				source_sentry_core::mutate::Current::Missing => None,
				source_sentry_core::mutate::Current::Present { oid, .. } => {
					Some(oid.clone())
				}
			};
			(entry.path.clone(), oid)
		})
		.collect();
	Stamp {
		sources,
		upstreams: Vec::new(),
	}
}
