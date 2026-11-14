//! The review session: a fresh evaluation, the queue it yields and the
//! reviews it records. Every accepted answer plans and applies one
//! attestation immediately; queue decisions always follow a fresh evaluation.
//! The line mode's loop is here; the full screen drives the same session in
//! the reviewer's own order.
//!
//! Each answer's writes are kept as a [`Step`], so that the newest answer can
//! be taken back: its files are restored through a write plan that expects
//! them exactly as the answer left them, and refuses otherwise.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};

use super::state::Key as DocumentId;
use source_sentry_core::mutate::{
	AttestRequest, DocumentDiff, Expectation, MutateError, Selection, attest,
	diff,
};
use source_sentry_core::ports::{
	Canonical, PlannedWrite, RepoPath, Snapshot as _, WritePlan,
};

use source_sentry_git::{ApplyError, WorkingTreeSnapshot};

use super::closing::{Closing, Part};
use super::opening::Opening;
use super::state::{Checkpoint, State, changed_sources};
use super::{Answer, Batch, Bulk, Interface, View};
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

/// One answer's writes, kept so that the answer can be taken back.
pub(super) struct Step {
	/// When the answer was recorded.
	pub(super) at: DateTime<Utc>,
	/// The reviews it recorded, and whether each recorded a reread: one, or
	/// several for an answer given to many documents at once.
	pub(super) reviews: Vec<(DocumentId, bool)>,
	/// How many sources it covered, when it reviewed some of a document's
	/// sources alone.
	pub(super) sources: Option<usize>,
	/// Each file it wrote, with the contents before the answer and after it.
	/// A file the answer created has nothing before, and cannot be restored.
	files: BTreeMap<RepoPath, (Option<Vec<u8>>, Vec<u8>)>,
}

/// What an attempt to take back the newest answer did.
pub(super) enum Undone {
	/// The session holds no answer to take back.
	Nothing,
	/// The answer was taken back: these reviews no longer stand.
	Withdrawn(Vec<(DocumentId, bool)>),
	/// This file is no longer as the answer left it, so nothing was restored.
	Changed(String),
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
	/// Whether the line mode also asks about documents that are not due yet.
	all: bool,
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
	/// The answers behind those reviews, oldest first, for taking back.
	steps: Vec<Step>,
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
			all: args.all,
			reviewer: super::super::person(workspace, args.reviewer.as_deref())?,
			state,
			snapshot,
			initial,
			answered: BTreeMap::new(),
			seen: BTreeMap::new(),
			recorded: Vec::new(),
			steps: Vec::new(),
			reasons: BTreeMap::new(),
			finished: 0,
			started: std::time::Instant::now(),
		})
	}

	/// Rebuilds the evidence after an answer or edit.
	pub(super) fn refresh(&mut self) -> Result<(), Failure> {
		self.reload(true)
	}

	/// Evaluates again. `noted` says whether a document whose review state
	/// changed is told why: taking an answer back changes that state on
	/// purpose, so only a source that really changed is explained then.
	fn reload(&mut self, noted: bool) -> Result<(), Failure> {
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
				} else if noted && !previous.pending && document.pending {
					self.reasons.insert(
						id.clone(),
						format!(
							"Started needing review during this session: {}.",
							document.facts
						),
					);
				} else if noted
					&& document.pending
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

	/// The remaining eligible queue in the current evaluation: the due
	/// documents, and with `--all` those not due yet.
	fn queue(&self) -> Result<Vec<DocumentId>, Failure> {
		self.state.queue(&self.selected, &self.answered, self.all)
	}

	/// Every eligible document that needs review, due or not, in queue
	/// order, whatever was answered before: the full screen lets the
	/// reviewer return to any, and marks those not due yet.
	pub(super) fn pending(&self) -> Result<Vec<DocumentId>, Failure> {
		self.state.queue(&self.selected, &BTreeMap::new(), true)
	}

	/// Whether `id` is due, or waits on an upstream document that is.
	pub(super) fn is_due(&self, id: &DocumentId) -> bool {
		self
			.state
			.documents
			.get(id)
			.is_some_and(|document| document.due)
	}

	/// The confirmation of `a` for `comparisons`: the due documents first,
	/// then those not due yet, with how many of each the reviewer has not
	/// seen.
	pub(super) fn batch(
		&self,
		comparisons: Vec<(DocumentId, DocumentDiff)>,
	) -> Batch {
		let (due, later): (Vec<_>, Vec<_>) =
			comparisons.into_iter().partition(|(id, _)| self.is_due(id));
		Batch {
			unseen: self.unseen(&due),
			unseen_later: self.unseen(&later),
			due,
			later,
		}
	}

	/// The comparison of `id` against the snapshot the latest evaluation
	/// read. It skips the reading that [`Self::compare`] repeats, so the full
	/// screen can show a document at once; a write still checks every
	/// expectation against a fresh snapshot, and refuses what has changed.
	pub(super) fn compare_evaluated(
		&self,
		id: &DocumentId,
	) -> Result<DocumentDiff, Failure> {
		if self.state.inventories.contains_key(&id.project) {
			self.compare_selected(id, &Selection::All)
		} else {
			self.compare(id)
		}
	}

	/// The comparison of the selected sources of `id` alone, against the
	/// snapshot the latest evaluation read, for a review that covers only
	/// them. Its expectations are those a write of that selection checks.
	pub(super) fn compare_selected(
		&self,
		id: &DocumentId,
		selection: &Selection,
	) -> Result<DocumentDiff, Failure> {
		let Some(inventory) = self.state.inventories.get(&id.project) else {
			return Err(Failure::new(
				exit::OPERATIONAL,
				"the document's project was not evaluated",
			));
		};
		diff(
			&self.snapshot,
			&self.workspace.repository,
			inventory,
			id,
			selection,
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
		self.write_selected(id, compared, &Selection::All, reread)
	}

	/// Writes a review of the selected sources alone, as `attest` does with
	/// `--source`. `compared` must be the comparison of that selection.
	/// Otherwise as [`Self::write`].
	pub(super) fn write_selected(
		&mut self,
		id: &DocumentId,
		compared: &DocumentDiff,
		selection: &Selection,
		reread: bool,
	) -> Result<Written, Failure> {
		let reread = reread || compared.sources.is_empty();
		let snapshot = self
			.workspace
			.repository
			.working_tree_snapshot()
			.map_err(|error| Failure::backend(&error))?;
		let (_, inventory) = load_project(self.workspace, &snapshot, &id.project)?;
		let at = super::super::now();
		let planned = match attest(
			&snapshot,
			&inventory,
			&AttestRequest {
				document: compared.document.clone(),
				selection: selection.clone(),
				expectations: Some(compared.expectations.clone()),
				reviewer: self.reviewer.clone(),
				at,
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
		// What each file holds now is what taking the answer back restores.
		let files = planned
			.plan
			.writes
			.iter()
			.map(|write| {
				let before = match write.expected {
					Some(_) => Some(
						snapshot
							.read(&write.path)
							.map_err(|error| Failure::backend(&error))?,
					),
					None => None,
				};
				Ok((write.path.clone(), (before, write.contents.clone())))
			})
			.collect::<Result<_, Failure>>()?;
		self
			.workspace
			.repository
			.apply(&planned.plan)
			.map_err(|error| Failure::apply(self.workspace, &error))?;
		self.recorded.push((id.clone(), reread));
		self.steps.push(Step {
			at,
			reviews: vec![(id.clone(), reread)],
			sources: match selection {
				Selection::All => None,
				Selection::Sources(names) => Some(names.len()),
				Selection::Declared(specs) => Some(specs.len()),
			},
			files,
		});
		self.finished += 1;
		Ok(Written::Recorded(reread))
	}

	/// The answers recorded in this session that still stand, oldest first.
	pub(super) fn steps(&self) -> &[Step] {
		&self.steps
	}

	/// Makes one answer of every step from the `from`-th onwards, for an
	/// answer given to several documents at once: taking it back takes all
	/// of its reviews back. A file keeps what it held before the first of
	/// them and after the last.
	pub(super) fn join(&mut self, from: usize) {
		if self.steps.len() <= from + 1 {
			return;
		}
		let mut later = self.steps.split_off(from + 1);
		let Some(first) = self.steps.last_mut() else {
			return;
		};
		first.sources = None;
		for step in later.drain(..) {
			first.reviews.extend(step.reviews);
			for (path, (before, after)) in step.files {
				match first.files.get_mut(&path) {
					Some(held) => held.1 = after,
					None => {
						first.files.insert(path, (before, after));
					}
				}
			}
		}
	}

	/// Takes back the newest answer of the session: every file it wrote
	/// returns to what it held before, and the working tree is evaluated
	/// again. Nothing is restored unless each of those files is still
	/// exactly as the answer left it, which the write itself checks again.
	///
	/// # Errors
	///
	/// Fails when the working tree cannot be read, when the restoring write
	/// fails otherwise than on a changed file, or when the evaluation fails.
	pub(super) fn undo(&mut self) -> Result<Undone, Failure> {
		let Some(step) = self.steps.last() else {
			return Ok(Undone::Nothing);
		};
		let snapshot = self
			.workspace
			.repository
			.working_tree_snapshot()
			.map_err(|error| Failure::backend(&error))?;
		let changed =
			|path: &RepoPath| Ok(Undone::Changed(self.workspace.display_path(path)));
		for (path, (before, after)) in &step.files {
			let now = snapshot.read(path).ok();
			if before.is_none() || now.as_ref() != Some(after) {
				return changed(path);
			}
		}
		let paths: Vec<RepoPath> = step.files.keys().cloned().collect();
		let oids = snapshot
			.canonical_oids(&paths)
			.map_err(|error| Failure::backend(&error))?;
		let mut writes = Vec::new();
		for ((path, (before, _)), oid) in step.files.iter().zip(oids) {
			let (Some(before), Canonical::Oid(oid)) = (before, oid) else {
				return changed(path);
			};
			writes.push(PlannedWrite {
				path: path.clone(),
				expected: Some(oid),
				contents: before.clone(),
			});
		}
		let plan = WritePlan {
			writes,
			deletes: Vec::new(),
		};
		match self.workspace.repository.apply(&plan) {
			Ok(()) => {}
			Err(ApplyError::Stale { path, .. }) => return changed(&path),
			Err(error) => return Err(Failure::apply(self.workspace, &error)),
		}
		let Some(step) = self.steps.pop() else {
			return Ok(Undone::Nothing);
		};
		let standing = self.recorded.len().saturating_sub(step.reviews.len());
		self.recorded.truncate(standing);
		self.finished = self.finished.saturating_sub(step.reviews.len());
		for (id, _) in &step.reviews {
			self.answered.remove(id);
		}
		self.reload(false)?;
		Ok(Undone::Withdrawn(step.reviews))
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
	/// The confirmation records the due documents by default, and offers to
	/// include those not due yet, which the queue holds with `--all`.
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
		let batch =
			self.batch(self.comparisons(queue, Some((&current, &view.compared)))?);
		let comparisons = match ui.confirm_all(&batch)? {
			None => return Ok(false),
			Some(Bulk::Leave) => return Ok(true),
			Some(Bulk::Due) => batch.due,
			Some(Bulk::Everything) => {
				batch.due.into_iter().chain(batch.later).collect()
			}
		};
		// One answer recorded them all, so they form one step.
		let from = self.steps.len();
		for (id, compared) in comparisons {
			if self.queue()?.contains(&id)
				&& !self.record(&id, &compared, false, ui)?
			{
				break;
			}
		}
		self.join(from);
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
