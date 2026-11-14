//! Fresh evaluations, dependency ordering, and answered-document checkpoints.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use source_sentry_core::claims::Inventory;
use source_sentry_core::evaluate::{DocumentState, SourceReport, check};
use source_sentry_core::model::{
	DocumentId, Fact, ObjectId, Outcome, RepoPath,
};
use source_sentry_core::policy::Threshold;
use source_sentry_git::WorkingTreeSnapshot;

use crate::exit::{self, Failure};
use crate::workspace::{Workspace, load_project};
use source_sentry_core::config::{project_of, project_roots};
use source_sentry_core::ports::Snapshot;

/// A document identity qualified by its owning project.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Key {
	/// The document's path or remote identity.
	pub(super) document: DocumentId,
	/// Its project root in the repository.
	pub(super) project: RepoPath,
}

impl Key {
	/// Qualifies a local path by ownership and a remote identity by selection.
	pub(super) fn new(
		document: DocumentId,
		roots: &BTreeSet<RepoPath>,
		selected: &RepoPath,
	) -> Self {
		let project = match &document {
			DocumentId::Local(path) => project_of(roots, path),
			DocumentId::Remote(_) => project_of(roots, selected),
		};
		Self { document, project }
	}

	/// Displays a remote project's name when needed to disambiguate identities.
	pub(super) fn display(&self, workspace: &Workspace) -> String {
		let shown = workspace.display_document(&self.document);
		if matches!(self.document, DocumentId::Remote(_)) && !self.project.is_root()
		{
			format!(
				"{shown} (project {})",
				workspace.display_path(&self.project)
			)
		} else {
			shown
		}
	}
}

/// Allows ordinary document I/O to use the underlying identity explicitly.
impl std::ops::Deref for Key {
	type Target = DocumentId;
	fn deref(&self) -> &DocumentId {
		&self.document
	}
}

/// The current canonical states of every source, including pattern removals.
type Sources = BTreeMap<RepoPath, Option<ObjectId>>;

/// Evidence relevant to whether an answered document needs another review.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Stamp {
	/// Direct source contents and membership.
	pub(super) sources: Sources,
	/// Direct upstream documents that still require review.
	pub(super) upstreams: Vec<RepoPath>,
}

/// A live document in a completed, valid evaluation.
pub(super) struct Document {
	/// Its outcome in the selected context.
	outcome: Outcome,
	/// Whether its facts require review, independently of the outcome.
	pub(super) pending: bool,
	/// Whether it is due, or one of its upstream documents is: a pending
	/// document that is not due can wait for its review day.
	pub(super) due: bool,
	/// Its current facts as people read them, for the prompt and closing
	/// summary: `stale (review by 2026-11-20)`.
	pub(super) facts: String,
	/// The evidence for suppressing unchanged repeat questions.
	pub(super) stamp: Stamp,
	/// Each source file as the evaluation found it: its state, and when it
	/// came to differ from its review.
	pub(super) reports: Vec<SourceReport>,
	/// Every transitive upstream, used to order equal outcomes.
	ancestors: BTreeSet<Key>,
}

impl Document {
	/// Its outcome in the selected context.
	pub(super) fn outcome(&self) -> Outcome {
		self.outcome
	}
}

/// The live documents of a fresh evaluation. Sealed records never enter it.
pub(super) struct State {
	/// Projects ordered by their default because the requested context is absent.
	pub(super) fallbacks: Vec<RepoPath>,
	/// Live documents, sorted by repository identity.
	pub(super) documents: BTreeMap<Key, Document>,
	/// Every tracked identity, including records that cannot be queued.
	pub(super) tracked: BTreeSet<Key>,
	/// The projects the evaluation traversed, by their roots.
	pub(super) projects: Vec<RepoPath>,
	/// Every context that a traversed project declares.
	pub(super) contexts: BTreeSet<String>,
	/// Each traversed project's inventory, as the evaluation discovered it.
	pub(super) inventories: BTreeMap<RepoPath, Inventory>,
}

impl State {
	/// Evaluates the working tree under `context`, refusing the same fatal
	/// diagnostics as `check` before presenting any document.
	#[cfg(test)]
	pub(super) fn load(
		workspace: &Workspace,
		context: &str,
	) -> Result<Self, Failure> {
		Self::load_with(workspace, context).map(|(state, _)| state)
	}

	/// Evaluates the working tree under `context`, refusing the same fatal
	/// diagnostics as `check` before presenting any document. Also returns
	/// the snapshot that was read, so that comparisons need not read the
	/// repository again.
	pub(super) fn load_with<'a>(
		workspace: &'a Workspace,
		context: &str,
	) -> Result<(Self, WorkingTreeSnapshot<'a>), Failure> {
		let snapshot = workspace
			.repository
			.working_tree_snapshot()
			.map_err(|error| Failure::backend(&error))?;
		let roots = project_roots(
			&snapshot.paths().map_err(|error| Failure::backend(&error))?,
		);
		let start = project_of(&roots, &workspace.scope);
		let mut state = Self {
			fallbacks: Vec::new(),
			documents: BTreeMap::new(),
			tracked: BTreeSet::new(),
			projects: Vec::new(),
			contexts: BTreeSet::new(),
			inventories: BTreeMap::new(),
		};
		for project in roots.iter().filter(|project| project.starts_with(&start)) {
			let one = Self::project(workspace, &snapshot, context, project)?;
			state.fallbacks.extend(one.fallbacks);
			state.documents.extend(one.documents);
			state.tracked.extend(one.tracked);
			state.projects.extend(one.projects);
			state.contexts.extend(one.contexts);
			state.inventories.extend(one.inventories);
		}
		let edges: BTreeMap<_, _> = state
			.documents
			.iter()
			.map(|(key, document)| (key.clone(), document.ancestors.clone()))
			.collect();
		for document in state.documents.values_mut() {
			let mut visit: Vec<_> = document.ancestors.iter().cloned().collect();
			while let Some(next) = visit.pop() {
				for upstream in edges.get(&next).into_iter().flatten() {
					if document.ancestors.insert(upstream.clone()) {
						visit.push(upstream.clone());
					}
				}
			}
		}
		Ok((state, snapshot))
	}

	/// Evaluates one project's live documents under its own declared context.
	fn project<S: Snapshot>(
		workspace: &Workspace,
		snapshot: &S,
		requested: &str,
		project: &RepoPath,
	) -> Result<Self, Failure> {
		let (configs, inventory) = load_project(workspace, snapshot, project)?;
		let fallback = !configs.is_declared_context(requested);
		let context = if fallback { "default" } else { requested };
		let evaluation = check(
			snapshot,
			&workspace.repository,
			&configs,
			&inventory,
			super::super::now(),
			Some(context),
		)
		.map_err(|error| Failure::backend(&error))?;
		valid(workspace, &evaluation)?;
		let mut documents = BTreeMap::new();
		for subject in &evaluation.subjects {
			let Some(document) = subject.document() else {
				continue;
			};
			let DocumentState::Live(live) = &document.state else {
				continue;
			};
			let outcome = subject.outcome().ok_or_else(|| {
				Failure::new(
					exit::CONFIGURATION,
					format!(
						"cannot judge `{}` under context `{context}`",
						workspace.display_document(&document.document)
					),
				)
			})?;
			let mut ancestors = BTreeSet::new();
			let mut visit = vec![document.document.clone()];
			while let Some(next) = visit.pop() {
				for path in inventory.upstreams_of(&next) {
					let upstream = DocumentId::Local(path.clone());
					if ancestors.insert(upstream.clone()) {
						visit.push(upstream);
					}
				}
			}
			documents.insert(
				Key::new(
					document.document.clone(),
					configs.projects(),
					configs.project(),
				),
				Document {
					outcome,
					reports: live.sources.clone(),
					pending: subject.facts.iter().any(|fact| {
						matches!(
							fact,
							Fact::Stale
								| Fact::Due
								| Fact::Overdue
								| Fact::Unattested
								| Fact::UpstreamStale
								| Fact::UpstreamDue
						)
					}),
					due: subject.facts.iter().any(|fact| {
						matches!(fact, Fact::Due | Fact::Overdue | Fact::UpstreamDue)
					}),
					facts: crate::describe::facts(subject),
					stamp: Stamp {
						sources: live
							.sources
							.iter()
							.map(|source| (source.path.clone(), source.current.clone()))
							.collect(),
						upstreams: live.stale_upstreams.clone(),
					},
					ancestors: ancestors
						.into_iter()
						.chain(live.stale_upstreams.iter().cloned().map(DocumentId::Local))
						.map(|document| {
							Key::new(document, configs.projects(), configs.project())
						})
						.collect(),
				},
			);
		}
		Ok(Self {
			fallbacks: fallback.then(|| project.clone()).into_iter().collect(),
			documents,
			tracked: inventory
				.claims
				.iter()
				.map(|claim| {
					Key::new(
						claim.document.clone(),
						configs.projects(),
						configs.project(),
					)
				})
				.collect(),
			projects: vec![project.clone()],
			contexts: configs.contexts().keys().cloned().collect(),
			inventories: BTreeMap::from([(project.clone(), inventory)]),
		})
	}

	/// How many of `queue`'s documents have each outcome, worst first.
	pub(super) fn outcomes(&self, queue: &[Key]) -> Vec<(Outcome, usize)> {
		let mut counts = BTreeMap::new();
		for id in queue {
			if let Some(document) = self.documents.get(id) {
				*counts.entry(document.outcome).or_insert(0_usize) += 1;
			}
		}
		counts.into_iter().rev().collect()
	}

	/// Orders eligible documents by decreasing outcome, then topologically
	/// within each outcome, taking the first available path at each step.
	/// A pending document that is not due is eligible only when `later`
	/// says so, or when it was selected by name.
	pub(super) fn queue(
		&self,
		selected: &BTreeSet<Key>,
		answered: &BTreeMap<Key, Checkpoint>,
		later: bool,
	) -> Result<Vec<Key>, Failure> {
		let mut rest: BTreeSet<_> = self
			.documents
			.iter()
			.filter(|(id, document)| {
				document.pending
					&& (document.due || later || selected.contains(*id))
					&& (selected.is_empty() || selected.contains(*id))
					&& !answered
						.get(*id)
						.is_some_and(|answer| answer.suppresses(document))
			})
			.map(|(id, _)| id.clone())
			.collect();
		let mut queue = Vec::new();
		while !rest.is_empty() {
			let worst = rest.iter().map(|id| self.documents[id].outcome).max();
			let next = rest.iter().find(|id| {
				let document = &self.documents[*id];
				Some(document.outcome) == worst
					&& !document.ancestors.iter().any(|upstream| {
						rest.contains(upstream)
							&& Some(self.documents[upstream].outcome) == worst
					})
			});
			let Some(next) = next.cloned() else {
				return Err(Failure::new(
					exit::CONFIGURATION,
					"cannot order the review queue: document dependencies contain a cycle",
				));
			};
			rest.remove(&next);
			queue.push(next);
		}
		Ok(queue)
	}
}

/// Why a document's unchanged state must not be presented again.
pub(super) enum Checkpoint {
	/// `n` suppresses another question until a direct source changes.
	Skipped(Stamp),
	/// An attestation suppresses its resulting state, even if still pending.
	Reviewed(Stamp),
}

impl Checkpoint {
	/// Whether another prompt would repeat the answer without new evidence.
	fn suppresses(&self, document: &Document) -> bool {
		match self {
			Self::Skipped(stamp) => stamp.sources == document.stamp.sources,
			Self::Reviewed(stamp) => stamp == &document.stamp,
		}
	}
}

/// The source paths whose contents or membership differ between checkpoints.
pub(super) fn changed_sources(
	old: &Sources,
	new: &Sources,
) -> BTreeSet<RepoPath> {
	old
		.keys()
		.chain(new.keys())
		.filter(|path| old.get(*path) != new.get(*path))
		.cloned()
		.collect()
}

/// Refuses fatal diagnostics before a project contributes to the review queue.
fn valid(
	workspace: &Workspace,
	evaluation: &source_sentry_core::evaluate::Evaluation,
) -> Result<(), Failure> {
	let code = evaluation.exit_code(Threshold::Warn).code();
	if code >= exit::CONFIGURATION {
		let mut message = String::from("cannot review this working tree:");
		for diagnostic in evaluation
			.diagnostics
			.iter()
			.filter(|diagnostic| diagnostic.severity().is_fatal())
		{
			let _ = write!(
				message,
				"\n  {}:{}: {}: {}",
				workspace.display_path(&diagnostic.location.file),
				diagnostic.location.line.unwrap_or(1),
				diagnostic.kind.code(),
				diagnostic.message
			);
		}
		return Err(Failure::new(code, message));
	}
	Ok(())
}
