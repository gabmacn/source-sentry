//! `track --from`: moving claims between tracking modes, lockfiles and
//! remote owners.

use std::collections::BTreeSet;

use super::edits::Edits;
use super::repo::{Placement, Repo, remote_lockfile_path};
use super::track::{
	CarriedTombstone, Target, location, lockfile_directory, on_file_allowed,
	prepare_lockfile, rediscovered,
};
use super::verify::verify;
use super::{MutateError, Planned};
use crate::claims::{Claim, DiagnosticKind, Inventory, Location};
use crate::config::ConfigSet;
use crate::evaluate::settings::EffectiveSettings;
use crate::model::{DocumentId, RepoPath};
use crate::ports::Snapshot;

/// What to move.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackFromRequest {
	/// The documents to move, all from the same placement. Splitting a
	/// lockfile moves every entry that the new lockfile takes over at once,
	/// and merging a nested lockfile into its parent moves all of its entries
	/// at once.
	pub documents: Vec<DocumentId>,
	/// Where the documents are tracked now, which must match.
	pub from: Placement,
	/// Where to track them.
	pub to: Target,
}

/// What `track --from` changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackFromSummary {
	/// Each moved document, in the order requested.
	pub moved: Vec<Moved>,
	/// Tombstones that move too: each moved entry's own tombstone for the
	/// same key, and those a new lockfile takes over.
	pub carried: Vec<CarriedTombstone>,
}

/// One moved document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Moved {
	/// The project whose declarations previously governed the document.
	pub project_before: RepoPath,
	/// The project receiving the declaration.
	pub project_after: RepoPath,
	/// Every rewritten source spelling and reviewed member.
	pub sources: Vec<super::RenamedSource>,
	/// The document.
	pub document: DocumentId,
	/// The file and field its declaration leaves.
	pub from: Location,
	/// The file and field its declaration joins.
	pub to: Location,
	/// Whether the plan creates the lockfile it joins.
	pub created: bool,
	/// How its effective settings change.
	pub settings: SettingsPreview,
}

/// A document's effective settings before and after a move.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsPreview {
	/// Project policy, format, time-zone and directory values that also change.
	pub configuration: Vec<super::ConfigurationChange>,
	/// The settings where the declaration is now.
	pub before: EffectiveSettings,
	/// The settings where it moves to.
	pub after: EffectiveSettings,
	/// The settings whose values change, in this order: classes, owners,
	/// grace, cadence. A change of origin alone is not listed.
	pub changed: Vec<Setting>,
}

/// One effective setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Setting {
	/// The document's classes.
	Classes,
	/// Its owners.
	Owners,
	/// Its grace period.
	Grace,
	/// Its cadence.
	Cadence,
}

/// The errors about a document that a move tolerates: the two it exists to
/// fix, an entry outside its lockfile's authority and an on-file document
/// used as a source, and problems with its sources and
/// classes, which the declaration carries along unchanged.
const TOLERATED: &[DiagnosticKind] = &[
	DiagnosticKind::OutsideAuthority,
	DiagnosticKind::OnFileSource,
	DiagnosticKind::MissingSource,
	DiagnosticKind::InvalidPattern,
	DiagnosticKind::SelfDependency,
	DiagnosticKind::UndeclaredClass,
	DiagnosticKind::DependencyCycle,
];

/// Plans moving claims between tracking modes, local lockfiles or remote
/// owners.
///
/// Each declaration moves unchanged: its identity, attestations, reviews,
/// `completed` and seal are preserved. The old location loses the entry and
/// the new one gains it, in one plan. Between lockfiles, an entry's own
/// tombstone for the same key moves with it. The summary previews how each
/// document's effective defaults change, computed with
/// [`EffectiveSettings`] before and after.
///
/// A lockfile that the move leaves with no documents and no tombstones is
/// deleted, as [`WritePlan::deletes`](crate::ports::WritePlan::deletes)
/// lists. Moving every entry of a nested lockfile to an explicit
/// ancestor directory therefore merges it into that directory's lockfile,
/// together with its tombstones.
///
/// # Errors
///
/// Refuses, planning nothing:
///
/// - an empty or repeated document ([`MutateError::NothingToMove`],
///   [`MutateError::RepeatedDocument`]);
/// - an untracked or doubly claimed document, one that is not tracked at
///   `from` ([`MutateError::WrongOrigin`]), and one with configuration or
///   integrity errors other than those a move carries or fixes;
/// - a target that does not fit the document's kind, or where it already
///   is ([`MutateError::SamePlacement`]);
/// - a record moved on-file ([`MutateError::RecordToOnFile`]), and
///   on-file tracking of a document whose adapter is not `md-utf8`, or that
///   is a source of another document;
/// - a lockfile directory that does not contain the document; one with a
///   nearer lockfile in between that the move does not empty
///   ([`MutateError::NearerLockfile`]); and a split or merge that would
///   leave entries behind ([`MutateError::StrandedEntries`]);
/// - a tombstone that would collide with another
///   ([`MutateError::TombstoneConflict`]);
/// - a change that would introduce a configuration or integrity error.
///
/// Fails when the backend fails.
pub fn track_from<S: Snapshot>(
	snapshot: &S,
	configs: &ConfigSet,
	inventory: &Inventory,
	request: &TrackFromRequest,
) -> Result<Planned<TrackFromSummary>, MutateError> {
	let mut combined = inventory.clone();
	let targets: BTreeSet<_> = request
		.documents
		.iter()
		.map(|document| match (&request.to, document) {
			(Target::Remote { project, .. }, _) => project.clone(),
			(_, DocumentId::Local(path)) => {
				crate::config::project_of(&inventory.projects, path)
			}
			(_, DocumentId::Remote(_)) => configs.project().clone(),
		})
		.collect();
	for project in targets
		.iter()
		.filter(|project| *project != configs.project())
	{
		let target_configs = ConfigSet::load_project(snapshot, project)?;
		combined.extend(crate::claims::discover(snapshot, &target_configs)?);
	}
	let repo = Repo::new(snapshot, &combined)?;
	if request.documents.is_empty() {
		return Err(MutateError::NothingToMove);
	}
	let mut moving = BTreeSet::new();
	for document in &request.documents {
		if !moving.insert(document.clone()) {
			return Err(MutateError::RepeatedDocument {
				document: document.clone(),
			});
		}
	}
	let mut edits = Edits::default();
	let mut carried = Vec::new();
	let mut moved = Vec::new();
	let mut targets = Vec::new();
	for document in &request.documents {
		let mut step = Step {
			repo: &repo,
			configs,
			edits: &mut edits,
			carried: &mut carried,
			moving: &moving,
		};
		let (one, target) = step.move_one(document, request)?;
		moved.push(one);
		targets.push(target);
	}
	let plan = edits.finish(&repo)?;
	let after = verify(&repo, &plan)?;
	for (one, target) in moved.iter().zip(&targets) {
		rediscovered(&after, &one.document, target, &one.to)?;
	}
	Ok(Planned {
		plan,
		summary: TrackFromSummary { moved, carried },
	})
}

/// The state that moving each document shares.
struct Step<'s, 'a, S> {
	repo: &'s Repo<'a, S>,
	configs: &'s ConfigSet,
	edits: &'s mut Edits,
	carried: &'s mut Vec<CarriedTombstone>,
	moving: &'s BTreeSet<DocumentId>,
}

impl<S: Snapshot> Step<'_, '_, S> {
	/// Plans moving `document`, and returns what it changes and where it
	/// goes.
	fn move_one(
		&mut self,
		document: &DocumentId,
		request: &TrackFromRequest,
	) -> Result<(Moved, Placement), MutateError> {
		let claim = origin(self.repo, document, &request.from)?;
		let actual = Placement::of(claim);
		self.repo.refuse_errors(document, TOLERATED)?;
		let project_before = crate::config::project_of(
			&self.repo.inventory.projects,
			&claim.declared_in,
		);
		let project_after = match (&request.to, document) {
			(Target::Remote { project, .. }, _) => project.clone(),
			(_, DocumentId::Local(path)) => {
				crate::config::project_of(&self.repo.inventory.projects, path)
			}
			(_, DocumentId::Remote(_)) => self.configs.project().clone(),
		};
		let destination_configs =
			ConfigSet::load_project(self.repo.snapshot, &project_after)?;
		super::settings::validate_classes(
			&destination_configs,
			&claim.declaration,
		)?;
		let (target, created) =
			self.destination(claim, &request.to, &destination_configs)?;
		if target == actual {
			return Err(MutateError::SamePlacement {
				document: document.clone(),
				placement: actual,
			});
		}
		let to = location(document, &target)?;
		let (declaration, sources) =
			rebase_claim(claim, &project_before, &project_after)?;
		let mut settings = super::settings::preview(
			super::settings::effective(
				self.configs,
				document,
				&actual,
				&claim.declaration,
				&claim.location(),
			),
			super::settings::effective(
				&destination_configs,
				document,
				&target,
				&declaration,
				&to,
			),
		);
		settings.configuration = super::settings::configuration_changes(
			(self.configs, document, &actual),
			(&destination_configs, document, &target),
		)?;
		if let (DocumentId::Remote(identity), Placement::Remote { project, owner }) =
			(document, &target)
			&& self
				.edits
				.remote(self.repo, project, owner)?
				.documents
				.contains_key(identity)
		{
			return Err(MutateError::DoubleClaim {
				document: document.clone(),
				sites: vec![claim.location(), to],
			});
		}
		self.edits.set(self.repo, claim, None)?;
		self.carry_own_tombstones(document, &actual, &target)?;
		self
			.edits
			.place(self.repo, document, &target, Some(declaration))?;
		Ok((
			Moved {
				project_before,
				project_after,
				sources,
				document: document.clone(),
				from: claim.location(),
				to,
				created,
				settings,
			},
			target,
		))
	}

	/// Where `claim` moves for `to`, and whether the plan creates that
	/// lockfile.
	fn destination(
		&mut self,
		claim: &Claim,
		to: &Target,
		configs: &ConfigSet,
	) -> Result<(Placement, bool), MutateError> {
		match (&claim.document, to) {
			(DocumentId::Local(path), Target::OnFile) => {
				if claim.is_sealed() {
					return Err(MutateError::RecordToOnFile {
						document: claim.document.clone(),
					});
				}
				if !self.repo.tracked.contains(path) {
					return Err(MutateError::DocumentNotFound { path: path.clone() });
				}
				on_file_allowed(self.repo, configs, path)?;
				Ok((Placement::OnFile, false))
			}
			(DocumentId::Local(path), Target::Lockfile { directory }) => {
				let directory =
					lockfile_directory(self.repo, path, directory.as_ref(), self.moving)?;
				let created = prepare_lockfile(
					self.repo,
					self.edits,
					path,
					&directory,
					self.moving,
					self.carried,
				)?;
				Ok((Placement::Lockfile { directory }, created))
			}
			(DocumentId::Remote(_), Target::Remote { project, owner }) => {
				let path = remote_lockfile_path(project, owner)?;
				Ok((
					Placement::Remote {
						project: project.clone(),
						owner: owner.clone(),
					},
					!self.repo.tracked.contains(&path),
				))
			}
			(document, target) => Err(MutateError::TargetMismatch {
				document: document.clone(),
				target: target.clone(),
			}),
		}
	}

	/// Moves every tombstone the old lockfile holds for the same key, if
	/// any, along with the entry. An entry that moves on-file, or
	/// from on-file, leaves tombstones where they are.
	fn carry_own_tombstones(
		&mut self,
		document: &DocumentId,
		from: &Placement,
		to: &Placement,
	) -> Result<(), MutateError> {
		let carried = CarriedTombstone {
			document: document.clone(),
			from: location(document, from)?.file,
			to: location(document, to)?.file,
		};
		let conflict = |carried: CarriedTombstone| MutateError::TombstoneConflict {
			document: carried.document,
			from: carried.from,
			to: carried.to,
		};
		match (document, from, to) {
			(
				DocumentId::Local(path),
				Placement::Lockfile { directory: old },
				Placement::Lockfile { directory: new },
			) => {
				let Some(tombstones) =
					self.edits.local(self.repo, old)?.retired.remove(path)
				else {
					return Ok(());
				};
				let target = self.edits.local(self.repo, new)?;
				if target.retired.contains_key(path) {
					return Err(conflict(carried));
				}
				target.retired.insert(path.clone(), tombstones);
			}
			(
				DocumentId::Remote(identity),
				Placement::Remote {
					project: old_project,
					owner: old,
				},
				Placement::Remote {
					project: new_project,
					owner: new,
				},
			) => {
				let Some(tombstones) = self
					.edits
					.remote(self.repo, old_project, old)?
					.retired
					.remove(identity)
				else {
					return Ok(());
				};
				let target = self.edits.remote(self.repo, new_project, new)?;
				if target.retired.contains_key(identity) {
					return Err(conflict(carried));
				}
				target.retired.insert(identity.clone(), tombstones);
			}
			_ => return Ok(()),
		}
		self.carried.push(carried);
		Ok(())
	}
}

/// Selects a declaration by its full placement, including its remote project.
fn origin<'a, S: Snapshot>(
	repo: &Repo<'a, S>,
	document: &DocumentId,
	from: &Placement,
) -> Result<&'a Claim, MutateError> {
	let claims: Vec<_> = repo
		.inventory
		.claims_of(document)
		.filter(|claim| Placement::of(claim) == *from)
		.collect();
	let claim = match claims.as_slice() {
		[claim] => *claim,
		[] => repo.claim(document)?,
		_ => {
			return Err(MutateError::DoubleClaim {
				document: document.clone(),
				sites: claims.iter().map(|claim| claim.location()).collect(),
			});
		}
	};
	let actual = Placement::of(claim);
	if actual != *from {
		return Err(MutateError::WrongOrigin {
			document: document.clone(),
			expected: Box::new(from.clone()),
			actual: Box::new(actual),
		});
	}
	Ok(claim)
}

/// Re-expresses sources and members while preserving every attestation.
fn rebase_claim(
	claim: &Claim,
	project_before: &RepoPath,
	project_after: &RepoPath,
) -> Result<(crate::model::Declaration, Vec<super::RenamedSource>), MutateError>
{
	let mut declaration = claim.declaration.clone();
	let mut sources = Vec::new();
	let old_path = match &claim.document {
		DocumentId::Local(path) => path,
		DocumentId::Remote(_) => project_before,
	};
	let new_path = match &claim.document {
		DocumentId::Local(path) => path,
		DocumentId::Remote(_) => project_after,
	};
	super::rename::sources::rebase(
		&mut declaration,
		project_before,
		project_after,
		old_path,
		new_path,
		&mut sources,
	)?;
	for source in &mut sources {
		source.document = claim.document.clone();
	}
	Ok((declaration, sources))
}
