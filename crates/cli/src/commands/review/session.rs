//! The review session: a fresh evaluation, the queue it yields and the
//! reviews it records. Every accepted answer plans and applies one
//! attestation immediately; queue decisions always follow a fresh evaluation.
//! The line mode's loop is here; the full screen drives the same session in
//! the reviewer's own order.

use std::collections::{BTreeMap, BTreeSet};

use super::state::Key as DocumentId;
use source_sentry_core::mutate::{
	AttestRequest, DocumentDiff, Expectation, MutateError, Selection, attest,
	diff,
};

use source_sentry_git::WorkingTreeSnapshot;

use super::closing::{Closing, Part};
use super::opening::Opening;
use super::state::{Checkpoint, State, changed_sources};
use super::{Answer, Interface, View};
use crate::cli::ReviewArgs;
use crate::exit::{self, Failure};
use crate::workspace::{Workspace, load_project};

/// What an attempt to record a review did.
pub(super) enum Written {
	/// The review was written; `true` when it recorded a reread.
	Recorded(bool),
	/// A source changed after it was shown, so nothing was written.
	Changed,
}

/// The refusal of a review whose sources changed after they were shown.
pub(super) const CHANGED: &str = "A source changed after it was shown; this review was not recorded. Showing the current changes again.";

/// Mutable session history; the working tree remains the source of truth.
pub(super) struct Session<'a> {
	/// The repository and path-display rules.
	pub(super) workspace: &'a Workspace,
	/// The context used for every evaluation.
	pub(super) context: String,
	/// Explicitly named documents, or an empty set for the whole repository.
	pub(super) selected: BTreeSet<DocumentId>,
	/// The reviewer, resolved as for `attest` when the session begins.
	pub(super) reviewer: Option<String>,
	/// The latest completed evaluation.
	pub(super) state: State,
	/// The snapshot that evaluation read.
	snapshot: WorkingTreeSnapshot<'a>,
	/// Documents needing review when the session began.
	initial: BTreeSet<DocumentId>,
	/// Answers that suppress unchanged repeat questions.
	answered: BTreeMap<DocumentId, Checkpoint>,
	/// The source versions whose comparisons the user has actually seen.
	seen: BTreeMap<DocumentId, Vec<Expectation>>,
	/// Applied reviews, including whether each recorded a reread.
	pub(super) recorded: Vec<(DocumentId, bool)>,
	/// The explanation of changes observed since the session began.
	pub(super) reasons: BTreeMap<DocumentId, String>,
	/// Answered positions; retries and editor visits keep their position.
	finished: usize,
	/// When the session began, for its closing.
	started: std::time::Instant,
}

impl<'a> Session<'a> {
	/// Loads and validates the starting evaluation and document selection.
	pub(super) fn new(
		workspace: &'a Workspace,
		args: &'a ReviewArgs,
	) -> Result<Self, Failure> {
		let (state, snapshot) = State::load_with(workspace, &args.context)?;
		let selected: BTreeSet<_> = args
			.documents
			.iter()
			.map(|argument| {
				workspace.document(argument).map(|document| {
					DocumentId::new(document, &workspace.projects, &workspace.scope)
				})
			})
			.collect::<Result<_, _>>()?;
		for id in &selected {
			if !state.tracked.contains(id) {
				return Err(Failure::new(
					exit::REFUSED,
					format!("`{}` is not tracked", id.display(workspace)),
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
			context: args.context.clone(),
			selected,
			reviewer: super::super::person(workspace, args.reviewer.as_deref())?,
			state,
			snapshot,
			initial,
			answered: BTreeMap::new(),
			seen: BTreeMap::new(),
			recorded: Vec::new(),
			reasons: BTreeMap::new(),
			finished: 0,
			started: std::time::Instant::now(),
		})
	}

	/// Rebuilds the evidence after an answer or edit.
	pub(super) fn refresh(&mut self) -> Result<(), Failure> {
		let (next, snapshot) = State::load_with(self.workspace, &self.context)?;
		self.snapshot = snapshot;
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

	/// Every eligible document that needs review, in queue order, whatever
	/// was answered before: the full screen lets the reviewer return to any.
	pub(super) fn pending(&self) -> Result<Vec<DocumentId>, Failure> {
		self.state.queue(&self.selected, &BTreeMap::new())
	}

	/// The comparison of `id` against the snapshot the latest evaluation
	/// read. It skips the reading that [`Self::compare`] repeats, so the full
	/// screen can show a document at once; a write still checks every
	/// expectation against a fresh snapshot, and refuses what has changed.
	pub(super) fn compare_evaluated(
		&self,
		id: &DocumentId,
	) -> Result<DocumentDiff, Failure> {
		let Some(inventory) = self.state.inventories.get(&id.project) else {
			return self.compare(id);
		};
		diff(
			&self.snapshot,
			&self.workspace.repository,
			inventory,
			id,
			&Selection::All,
		)
		.map_err(|error| Failure::mutate(self.workspace, &error))
	}

	/// Evaluates again under another context, which decides outcomes and
	/// therefore the queue's order.
	pub(super) fn set_context(&mut self, context: &str) -> Result<(), Failure> {
		(self.state, self.snapshot) = State::load_with(self.workspace, context)?;
		context.clone_into(&mut self.context);
		Ok(())
	}

	/// Notes that the reviewer has been shown this comparison.
	pub(super) fn see(&mut self, id: &DocumentId, compared: &DocumentDiff) {
		self.seen.insert(id.clone(), compared.expectations.clone());
	}

	/// The facts of the session ahead, given its starting queue.
	pub(super) fn opening(&self, queue: &[DocumentId]) -> Opening {
		Opening {
			reviewer: self.reviewer.clone(),
			context: self.context.clone(),
			projects: self.state.projects.len(),
			queued: queue.len(),
			tracked: if self.selected.is_empty() {
				self.state.tracked.len()
			} else {
				self.selected.len()
			},
			outcomes: self.state.outcomes(queue),
		}
	}

	/// Captures exactly the comparison whose expectations an answer approves.
	pub(super) fn compare(
		&self,
		id: &DocumentId,
	) -> Result<DocumentDiff, Failure> {
		let snapshot = self
			.workspace
			.repository
			.working_tree_snapshot()
			.map_err(|error| Failure::backend(&error))?;
		let (_, inventory) = load_project(self.workspace, &snapshot, &id.project)?;
		diff(
			&snapshot,
			&self.workspace.repository,
			&inventory,
			id,
			&Selection::All,
		)
		.map_err(|error| Failure::mutate(self.workspace, &error))
	}

	/// Writes one approved comparison. A source that changed after it was
	/// shown refuses the write and evaluates again; a written review still
	/// needs [`Self::settle`], so that a caller can report it first.
	pub(super) fn write(
		&mut self,
		id: &DocumentId,
		compared: &DocumentDiff,
		reread: bool,
	) -> Result<Written, Failure> {
		let reread = reread || compared.sources.is_empty();
		let snapshot = self
			.workspace
			.repository
			.working_tree_snapshot()
			.map_err(|error| Failure::backend(&error))?;
		let (_, inventory) = load_project(self.workspace, &snapshot, &id.project)?;
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
				self.refresh()?;
				return Ok(Written::Changed);
			}
			Err(error) => return Err(Failure::mutate(self.workspace, &error)),
		};
		self
			.workspace
			.repository
			.apply(&planned.plan)
			.map_err(|error| Failure::apply(self.workspace, &error))?;
		self.recorded.push((id.clone(), reread));
		self.finished += 1;
		Ok(Written::Recorded(reread))
	}

	/// Evaluates again after a written review, and remembers the resulting
	/// state so that the line mode does not ask the same question twice.
	pub(super) fn settle(&mut self, id: &DocumentId) -> Result<(), Failure> {
		self.refresh()?;
		if let Some(document) = self.state.documents.get(id)
			&& document.pending
		{
			self
				.answered
				.insert(id.clone(), Checkpoint::Reviewed(document.stamp.clone()));
		}
		Ok(())
	}

	/// Records one approved comparison for the line mode. A changed source
	/// refuses this answer and requests another prompt with a new comparison.
	fn record(
		&mut self,
		id: &DocumentId,
		compared: &DocumentDiff,
		reread: bool,
		ui: &mut impl Interface,
	) -> Result<bool, Failure> {
		match self.write(id, compared, reread)? {
			Written::Changed => {
				ui.message(CHANGED);
				Ok(false)
			}
			Written::Recorded(reread) => {
				ui.message(&self.recorded_notice(compared, reread));
				self.settle(id)?;
				Ok(true)
			}
		}
	}

	/// The sentence that reports a written review.
	pub(super) fn recorded_notice(
		&self,
		compared: &DocumentDiff,
		reread: bool,
	) -> String {
		format!(
			"Recorded {}{}.",
			self.workspace.display_document(&compared.document),
			if reread { " (reread)" } else { "" }
		)
	}

	/// The current comparison of every document in `queue`, with every source
	/// expectation frozen. `shown` is reused for the document on display.
	pub(super) fn comparisons(
		&self,
		queue: &[DocumentId],
		shown: Option<(&DocumentId, &DocumentDiff)>,
	) -> Result<Vec<(DocumentId, DocumentDiff)>, Failure> {
		queue
			.iter()
			.map(|id| {
				let compared = match shown {
					Some((current, compared)) if current == id => compared.clone(),
					_ => self.compare(id)?,
				};
				Ok((id.clone(), compared))
			})
			.collect()
	}

	/// How many of `comparisons` the reviewer has not seen as they are now.
	pub(super) fn unseen(
		&self,
		comparisons: &[(DocumentId, DocumentDiff)],
	) -> usize {
		comparisons
			.iter()
			.filter(|(id, compared)| !self.has_seen(id, compared))
			.count()
	}

	/// Whether the reviewer has been shown this comparison as it is now.
	pub(super) fn has_seen(
		&self,
		id: &DocumentId,
		compared: &DocumentDiff,
	) -> bool {
		self.seen.get(id) == Some(&compared.expectations)
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
		let current = DocumentId {
			document: view.compared.document.clone(),
			project: view.project.clone(),
		};
		let comparisons =
			self.comparisons(queue, Some((&current, &view.compared)))?;
		match ui.confirm_all(queue.len(), self.unseen(&comparisons))? {
			None => return Ok(false),
			Some(false) => return Ok(true),
			Some(true) => {}
		}
		for (id, compared) in comparisons {
			if self.queue()?.contains(&id)
				&& !self.record(&id, &compared, false, ui)?
			{
				break;
			}
		}
		Ok(true)
	}

	/// Ends the session: shows at once the part of the closing that the
	/// session itself knows, then evaluates the working tree a final time and
	/// shows what remains, so that a change made outside the session, such as
	/// an edit that makes other documents stale, is reported.
	///
	/// # Errors
	///
	/// Fails when the final evaluation fails; the first part is shown then.
	pub(super) fn finish(
		&mut self,
		mut show: impl FnMut(&Closing, Part),
	) -> Result<(), Failure> {
		show(&self.closing(), Part::Known);
		self.refresh()?;
		show(&self.closing(), Part::Evaluated);
		Ok(())
	}

	/// What the session did and what remains, from the latest evaluation,
	/// including documents outside an explicit selection.
	pub(super) fn closing(&self) -> Closing {
		Closing {
			reviewer: self.reviewer.clone(),
			recorded: self
				.recorded
				.iter()
				.map(|(id, reread)| (id.display(self.workspace), *reread))
				.collect(),
			remaining: self
				.state
				.documents
				.iter()
				.filter(|(_, document)| document.pending)
				.map(|(id, document)| {
					let mut line =
						format!("{}: {}", id.display(self.workspace), document.facts);
					if !self.initial.contains(id) {
						line.push_str("; started needing review during this session");
					}
					if !self.selected.is_empty() && !self.selected.contains(id) {
						line.push_str("; outside the selected documents");
					}
					line
				})
				.collect(),
			seconds: self.started.elapsed().as_secs(),
			check: format!("{} check", self.workspace.command_prefix()),
		}
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
	let starting = session.queue()?;
	if !starting.is_empty() {
		ui.open(&session.opening(&starting));
	}
	for project in &session.state.fallbacks {
		ui.message(&format!("Project {} does not declare context `{}`; ordering its documents by its own `default`.", workspace.display_path(project), args.context));
	}
	let mut retry = None;
	'session: loop {
		let mut queue = session.queue()?;
		if queue.is_empty() {
			session.refresh()?;
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
			project: id.project.clone(),
			compared: session.compare(&id)?,
			position: session.finished + 1,
			total: session.finished + queue.len(),
			reason: session.reasons.get(&id).cloned(),
			facts: session.state.documents[&id].facts.clone(),
			outcome: session.state.documents[&id].outcome(),
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
					session.refresh()?;
					retry = Some(id.clone());
					break;
				}
				Some(Answer::Next) => {
					session.finished += 1;
					session.refresh()?;
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
					if !session.record(&id, &view.compared, reread, ui)? {
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
	session.finish(|closing, part| ui.close(closing, part))
}
