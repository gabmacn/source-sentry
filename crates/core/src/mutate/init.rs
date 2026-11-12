//! `init`: previewing and planning adoption.

use std::collections::BTreeSet;

use super::edits::Edits;
use super::repo::{Placement, Repo};
use super::track::{
	CarriedTombstone, Registered, Target, TrackRequest, confirm, register,
};
use super::verify::verify;
use super::{MutateError, Planned};
use crate::claims::{Inventory, Location, Unclaimed};
use crate::config::ConfigSet;
use crate::model::{ClassName, DocumentId, RepoPath};
use crate::ports::{Snapshot, WritePlan};

/// What adoption starts from: the roots, what is tracked already, and what
/// could be.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InitPreview {
	/// Every root, sorted.
	pub roots: Vec<RepoPath>,
	/// Every existing claim, in the inventory's order.
	pub claims: Vec<ClaimedDocument>,
	/// Every unclaimed candidate, sorted by path, with the root whose scan
	/// found it.
	pub candidates: Vec<Unclaimed>,
}

/// One existing claim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaimedDocument {
	/// The document.
	pub document: DocumentId,
	/// Where its declaration lives.
	pub placement: Placement,
	/// The file and field of the declaration.
	pub declared_in: Location,
}

/// Which candidates to register.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Candidates {
	/// Every unclaimed candidate.
	#[default]
	All,
	/// These candidates only.
	Only(Vec<RepoPath>),
}

/// What to register during adoption.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InitRequest {
	/// The candidates to register.
	pub candidates: Candidates,
	/// Classes to assign to every registered document. Empty to inherit
	/// them from each root.
	pub classes: Vec<ClassName>,
	/// Owners to assign to every registered document. Empty to inherit them.
	pub owners: Vec<String>,
}

/// What `init` registers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InitSummary {
	/// Each registered document, sorted by path.
	pub registered: Vec<Registered>,
	/// Tombstones that new lockfiles take over.
	pub carried: Vec<CarriedTombstone>,
}

/// Previews adoption from the inventory alone: the roots, the existing
/// claims and the unclaimed candidates. It plans nothing.
#[must_use]
pub fn init_preview(configs: &ConfigSet, inventory: &Inventory) -> InitPreview {
	InitPreview {
		roots: configs.roots(),
		claims: inventory
			.claims
			.iter()
			.map(|claim| ClaimedDocument {
				document: claim.document.clone(),
				placement: Placement::of(claim),
				declared_in: claim.location(),
			})
			.collect(),
		candidates: inventory.unclaimed.clone(),
	}
}

/// Plans registering the chosen candidates as unattested lockfile entries
/// with no sources, through the same logic as `track`.
/// No review is ever recorded.
///
/// Each candidate goes into the nearest existing lockfile, or, when none
/// covers it, into a new lockfile at the root whose scan found it, so that a
/// root's candidates share one lockfile. With no candidate to register, the
/// plan is empty.
/// Series entries inherit through the same planner as `track`, using the
/// declarations present in the input inventory.
///
/// # Errors
///
/// Refuses, planning nothing, a path that is not an unclaimed candidate
/// ([`MutateError::NotACandidate`]), and any candidate that `track` would
/// refuse, such as one with configuration or integrity errors, an
/// undeclared class or an empty owner. Fails when the backend fails.
pub fn init<S: Snapshot>(
	snapshot: &S,
	configs: &ConfigSet,
	inventory: &Inventory,
	request: &InitRequest,
) -> Result<Planned<InitSummary>, MutateError> {
	let repo = Repo::new(snapshot, inventory)?;
	let chosen: Vec<&Unclaimed> = match &request.candidates {
		Candidates::All => inventory.unclaimed.iter().collect(),
		Candidates::Only(paths) => {
			let paths: BTreeSet<&RepoPath> = paths.iter().collect();
			paths
				.into_iter()
				.map(|path| {
					inventory
						.unclaimed
						.iter()
						.find(|candidate| &candidate.path == path)
						.ok_or_else(|| MutateError::NotACandidate { path: path.clone() })
				})
				.collect::<Result<_, _>>()?
		}
	};
	if chosen.is_empty() {
		return Ok(Planned {
			plan: WritePlan::default(),
			summary: InitSummary::default(),
		});
	}
	let mut edits = Edits::default();
	let mut carried = Vec::new();
	let mut pending = Vec::with_capacity(chosen.len());
	for candidate in chosen {
		let directory = repo
			.nearest_lockfile(&candidate.path)
			.unwrap_or_else(|| candidate.root.clone());
		let track = TrackRequest {
			document: DocumentId::Local(candidate.path.clone()),
			target: Target::Lockfile {
				directory: Some(directory),
			},
			placement_named: true,
			sources: Vec::new(),
			classes: request.classes.clone(),
			owners: request.owners.clone(),
			schedule: None,
			url: None,
		};
		pending.push(register(&repo, configs, &mut edits, &mut carried, &track)?);
	}
	let plan = edits.finish(&repo)?;
	let after = verify(&repo, &plan)?;
	let registered = pending
		.into_iter()
		.map(|pending| confirm(&after, pending))
		.collect::<Result<_, _>>()?;
	Ok(Planned {
		plan,
		summary: InitSummary {
			registered,
			carried,
		},
	})
}
