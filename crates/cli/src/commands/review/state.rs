//! Fresh evaluations, dependency ordering, and answered-document checkpoints.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use source_sentry_core::evaluate::{DocumentState, check};
use source_sentry_core::model::{
	DocumentId, Fact, ObjectId, Outcome, RepoPath,
};
use source_sentry_core::policy::Threshold;

use crate::exit::{self, Failure};
use crate::workspace::{Workspace, load};

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
	/// Its current facts, for the prompt and closing summary.
	pub(super) facts: String,
	/// The evidence for suppressing unchanged repeat questions.
	pub(super) stamp: Stamp,
	/// Every transitive upstream, used to order equal outcomes.
	ancestors: BTreeSet<DocumentId>,
}

/// The live documents of a fresh evaluation. Sealed records never enter it.
pub(super) struct State {
	/// Live documents, sorted by repository identity.
	pub(super) documents: BTreeMap<DocumentId, Document>,
	/// Every tracked identity, including records that cannot be queued.
	pub(super) tracked: BTreeSet<DocumentId>,
}

impl State {
	/// Evaluates the working tree under `context`, refusing the same fatal
	/// diagnostics as `check` before presenting any document.
	pub(super) fn load(
		workspace: &Workspace,
		context: &str,
	) -> Result<Self, Failure> {
		let snapshot = workspace
			.repository
			.working_tree_snapshot()
			.map_err(|error| Failure::backend(&error))?;
		let (configs, inventory) = load(workspace, &snapshot)?;
		let evaluation = check(
			&snapshot,
			&workspace.repository,
			&configs,
			&inventory,
			super::super::now(),
			Some(context),
		)
		.map_err(|error| Failure::backend(&error))?;
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
				document.document.clone(),
				Document {
					outcome,
					pending: subject.facts.iter().any(|fact| {
						matches!(
							fact,
							Fact::Stale
								| Fact::Overdue
								| Fact::Unattested
								| Fact::UpstreamStale
						)
					}),
					facts: subject
						.facts
						.iter()
						.map(|fact| fact.name())
						.collect::<Vec<_>>()
						.join(", "),
					stamp: Stamp {
						sources: live
							.sources
							.iter()
							.map(|source| (source.path.clone(), source.current.clone()))
							.collect(),
						upstreams: live.stale_upstreams.clone(),
					},
					ancestors,
				},
			);
		}
		Ok(Self {
			documents,
			tracked: inventory
				.claims
				.iter()
				.map(|claim| claim.document.clone())
				.collect(),
		})
	}

	/// Orders eligible documents by decreasing outcome, then topologically
	/// within each outcome, taking the first available path at each step.
	pub(super) fn queue(
		&self,
		selected: &BTreeSet<DocumentId>,
		answered: &BTreeMap<DocumentId, Checkpoint>,
	) -> Result<Vec<DocumentId>, Failure> {
		let mut rest: BTreeSet<_> = self
			.documents
			.iter()
			.filter(|(id, document)| {
				document.pending
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
