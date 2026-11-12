//! `track`: registering a document as unattested.
//!
//! The lockfile helpers here are shared with `track --from` and `init`.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use super::edits::Edits;
use super::repo::{
	Placement, Repo, is_own_file, lockfile_path, remote_lockfile_path,
};
use super::select::declared;
use super::verify::verify;
use super::{MutateError, Planned};
use crate::adapters::Adapter;
use crate::claims::{
	Claim, FRONTMATTER_FIELD, Inventory, Location, Mode, ResolvedSource,
	SourceGlob, entry_field,
};
use crate::config::ConfigSet;
use crate::evaluate::schedule::EffectiveSchedule;
use crate::model::{
	CadenceSetting, ClassName, Declaration, DocumentId, PathError, PathSource,
	PatternSource, RepoPath, Schedule, SourceDecl, resolve_rooted,
	resolve_source,
};
use crate::ports::{Canonical, Snapshot};

/// One source to declare.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceSpec {
	/// A single file: a leading `/` means the repository root, and any other
	/// path is relative to the document.
	Path(String),
	/// A glob, every match of which is a source.
	Pattern(String),
}

/// Where to register a document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
	/// In the document's own frontmatter. Only for documents whose adapter
	/// is `md-utf8`.
	OnFile,
	/// In a local lockfile.
	Lockfile {
		/// The lockfile's directory. By default, the nearest existing
		/// ancestor lockfile, or a new `source-sentry.lock` in the
		/// document's directory when there is none. An explicit directory
		/// must contain the document, with no nearer lockfile in between.
		directory: Option<RepoPath>,
	},
	/// In the owner lockfile `.source-sentry/remotes/<owner>.lock`, which is
	/// created when missing. Only for remote documents.
	Remote {
		/// The owner.
		owner: String,
	},
}

impl fmt::Display for Target {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::OnFile => Placement::OnFile.fmt(formatter),
			Self::Lockfile { directory: None } => {
				formatter.write_str("in a local lockfile")
			}
			Self::Lockfile {
				directory: Some(directory),
			} => Placement::Lockfile {
				directory: directory.clone(),
			}
			.fmt(formatter),
			Self::Remote { owner } => Placement::Remote {
				owner: owner.clone(),
			}
			.fmt(formatter),
		}
	}
}

/// What to register.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackRequest {
	/// The document: a local path, or a remote identity.
	pub document: DocumentId,
	/// Where to register it.
	pub target: Target,
	/// Whether the caller named the placement. For a document that is
	/// already tracked, `track` adds to its declaration where it is: always
	/// when the placement was not named, and only when `target` names that
	/// placement when it was.
	pub placement_named: bool,
	/// The sources to declare, unattested, in this order.
	pub sources: Vec<SourceSpec>,
	/// Classes to assign, which the repository config must declare. Empty
	/// to inherit them.
	pub classes: Vec<ClassName>,
	/// Owner identifiers. Empty to inherit them.
	pub owners: Vec<String>,
	/// A schedule that overrides the inherited one.
	pub schedule: Option<Schedule>,
	/// A link to a remote document. Remote documents only, and never part of
	/// the identity.
	pub url: Option<String>,
}

/// What `track` registers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackSummary {
	/// The registration.
	pub registered: Registered,
	/// Tombstones that a new lockfile takes over from the lockfile that
	/// listed them, because only the nearest lockfile may hold them.
	pub carried: Vec<CarriedTombstone>,
}

/// One registered document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Registered {
	/// The document.
	pub document: DocumentId,
	/// Where its declaration lives.
	pub placement: Placement,
	/// The file and field of the declaration.
	pub declared_in: Location,
	/// Whether the plan creates that lockfile, which makes its directory a
	/// root.
	pub created: bool,
	/// Whether the document was already tracked, so the plan adds to its
	/// declaration instead of registering it.
	pub extended: bool,
	/// The declaration. A new one carries no attestation and no review; an
	/// extended one keeps those it had, and its new sources are unattested.
	pub declaration: Declaration,
	/// What each declared pattern matches now.
	pub matches: Vec<PatternMatches>,
}

/// The files one pattern matches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PatternMatches {
	/// The pattern, as declared.
	pub pattern: String,
	/// The tracked files it matches, sorted.
	pub matches: Vec<RepoPath>,
}

/// A tombstone that moves with a lockfile change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CarriedTombstone {
	/// The retired document.
	pub document: DocumentId,
	/// The lockfile it leaves.
	pub from: RepoPath,
	/// The lockfile it joins.
	pub to: RepoPath,
}

/// Plans registering a document as unattested: its declared sources carry
/// no oids and no reviews, and the document has no `reviewed`.
/// Classes, owners and a schedule are optional.
///
/// The declaration goes in the document's frontmatter, in a local lockfile,
/// or in a remote owner's lockfile, as [`Target`] describes. A missing
/// lockfile is created. A new local lockfile nested in another one's
/// authority takes over that lockfile's tombstones below its directory.
///
/// # Errors
///
/// Refuses, planning nothing:
///
/// - a document that is already claimed ([`MutateError::AlreadyClaimed`]),
///   since that would be a double claim;
/// - a local document that the snapshot lacks, one of Source Sentry's own
///   files, or a document with configuration or integrity errors;
/// - a target that does not fit the document's kind, and a `url` for a
///   local document;
/// - on-file tracking of a document whose adapter is not `md-utf8`, or that
///   is a source of another document;
/// - a lockfile directory that does not contain the document, or with a
///   nearer lockfile in between, and a new lockfile that would
///   take authority over another lockfile's entries;
/// - a source path that does not resolve or is not tracked, a remote
///   document's source that is not `/`-rooted, a source that is
///   the document itself, a repeated source, and a new pattern that matches
///   nothing ([`MutateError::EmptyPattern`]);
/// - an undeclared class, an empty owner, an invalid owner name or an
///   invalid schedule;
/// - a change that would introduce any other configuration or integrity
///   error, such as a dependency cycle.
///
/// Fails when the backend fails.
pub fn track<S: Snapshot>(
	snapshot: &S,
	configs: &ConfigSet,
	inventory: &Inventory,
	request: &TrackRequest,
) -> Result<Planned<TrackSummary>, MutateError> {
	let repo = Repo::new(snapshot, inventory)?;
	let mut edits = Edits::default();
	let mut carried = Vec::new();
	let pending = register(&repo, configs, &mut edits, &mut carried, request)?;
	let plan = edits.finish(&repo)?;
	let after = verify(&repo, &plan)?;
	let registered = confirm(&after, pending)?;
	Ok(Planned {
		plan,
		summary: TrackSummary {
			registered,
			carried,
		},
	})
}

/// A registration planned in [`Edits`], to confirm once discovery has seen
/// the result.
pub(super) struct Pending {
	document: DocumentId,
	placement: Placement,
	declared_in: Location,
	created: bool,
	extended: bool,
	/// The patterns this plan declares, as written, which must match a file.
	/// Patterns declared before may have lost their matches since; that is
	/// for `check` to report, not a reason to refuse the plan.
	new_patterns: BTreeSet<String>,
}

/// The patterns among `sources`, as written.
fn patterns_of(sources: &[SourceDecl]) -> BTreeSet<String> {
	sources
		.iter()
		.filter_map(|source| match source {
			SourceDecl::Pattern(pattern) => Some(pattern.pattern.clone()),
			SourceDecl::Path(_) => None,
		})
		.collect()
}

/// Plans one registration in `edits`.
///
/// # Errors
///
/// Fails with the refusals of [`track`] that do not need discovery of the
/// result.
pub(super) fn register<S: Snapshot>(
	repo: &Repo<'_, S>,
	configs: &ConfigSet,
	edits: &mut Edits,
	carried: &mut Vec<CarriedTombstone>,
	request: &TrackRequest,
) -> Result<Pending, MutateError> {
	let document = &request.document;
	if let [claim] = repo.inventory.claims_of(document).collect::<Vec<_>>()[..]
		&& adds_something(request)
		&& (!request.placement_named
			|| names(&request.target, &Placement::of(claim)))
	{
		return extend(repo, configs, edits, request, claim);
	}
	let (placement, created) = match (document, &request.target) {
		(DocumentId::Local(path), Target::OnFile) => {
			unclaimed_local(repo, path)?;
			on_file_allowed(repo, configs, path)?;
			(Placement::OnFile, false)
		}
		(DocumentId::Local(path), Target::Lockfile { directory }) => {
			unclaimed_local(repo, path)?;
			let moving = BTreeSet::from([document.clone()]);
			let directory =
				lockfile_directory(repo, path, directory.as_ref(), &moving)?;
			let created =
				prepare_lockfile(repo, edits, path, &directory, &moving, carried)?;
			(Placement::Lockfile { directory }, created)
		}
		(DocumentId::Remote(_), Target::Remote { owner }) => {
			unclaimed(repo, document)?;
			let path = remote_lockfile_path(owner)?;
			let created = !repo.tracked.contains(&path);
			(
				Placement::Remote {
					owner: owner.clone(),
				},
				created,
			)
		}
		(document, target) => {
			return Err(MutateError::TargetMismatch {
				document: document.clone(),
				target: target.to_string(),
			});
		}
	};
	let declaration = declaration(repo, configs, request)?;
	let declared_in = location(document, &placement)?;
	let new_patterns = patterns_of(&declaration.sources);
	edits.place(repo, document, &placement, Some(declaration))?;
	Ok(Pending {
		document: document.clone(),
		placement,
		declared_in,
		created,
		extended: false,
		new_patterns,
	})
}

/// Whether `request` asks for anything beyond the registration itself. A
/// bare `track` of a tracked document is still refused as a double claim.
fn adds_something(request: &TrackRequest) -> bool {
	!request.sources.is_empty()
		|| !request.classes.is_empty()
		|| !request.owners.is_empty()
		|| request.schedule.is_some()
		|| request.url.is_some()
}

/// Whether `target` names `placement`. `--lockfile` without a directory
/// names any local lockfile.
fn names(target: &Target, placement: &Placement) -> bool {
	match (target, placement) {
		(Target::OnFile, Placement::OnFile)
		| (Target::Lockfile { directory: None }, Placement::Lockfile { .. }) => true,
		(
			Target::Lockfile {
				directory: Some(named),
			},
			Placement::Lockfile { directory },
		) => named == directory,
		(Target::Remote { owner: named }, Placement::Remote { owner }) => {
			named == owner
		}
		_ => false,
	}
}

/// Plans adding `request`'s sources, classes, owners, schedule and url to
/// the declaration of `claim`, where it is.
///
/// Reviews and attestations are kept, and the new sources are unattested,
/// so the document is stale until they are reviewed. Classes and owners are
/// added once each. A grace or cadence replaces only what it names.
///
/// # Errors
///
/// Fails as registration does for the new values, with
/// [`MutateError::DuplicateSource`] for a source already declared, and with
/// [`MutateError::Sealed`] for a record. An on-file declaration that other
/// documents read is extended like any other, as `attest` writes to it.
fn extend<S: Snapshot>(
	repo: &Repo<'_, S>,
	configs: &ConfigSet,
	edits: &mut Edits,
	request: &TrackRequest,
	claim: &Claim,
) -> Result<Pending, MutateError> {
	let document = &request.document;
	repo.refuse_errors(document, &[])?;
	if claim.is_sealed() {
		return Err(MutateError::Sealed {
			document: document.clone(),
		});
	}
	let placement = Placement::of(claim);
	let addition = declaration(repo, configs, request)?;
	let new_patterns = patterns_of(&addition.sources);
	let mut declaration = claim.declaration.clone();
	for source in addition.sources {
		if let Some(first) = declaration
			.sources
			.iter()
			.find(|existing| same_source(document, existing, &source))
		{
			let (first, pattern) = match first {
				SourceDecl::Path(path) => (path.path.clone(), false),
				SourceDecl::Pattern(pattern) => (pattern.pattern.clone(), true),
			};
			return Err(MutateError::DuplicateSource {
				document: document.clone(),
				first,
				declared: match &source {
					SourceDecl::Path(path) => path.path.clone(),
					SourceDecl::Pattern(pattern) => pattern.pattern.clone(),
				},
				pattern,
			});
		}
		declaration.sources.push(source);
	}
	for class in addition.classes {
		if !declaration.classes.contains(&class) {
			declaration.classes.push(class);
		}
	}
	for owner in addition.owners {
		if !declaration.owners.contains(&owner) {
			declaration.owners.push(owner);
		}
	}
	if let Some(added) = addition.schedule {
		let schedule = declaration.schedule.get_or_insert(Schedule {
			cadence: CadenceSetting::Inherit,
			grace: None,
		});
		if added.grace.is_some() {
			schedule.grace = added.grace;
		}
		if !added.cadence.is_inherit() {
			schedule.cadence = added.cadence;
		}
	}
	if addition.url.is_some() {
		declaration.url = addition.url;
	}
	let declared_in = claim.location();
	edits.place(repo, document, &placement, Some(declaration))?;
	Ok(Pending {
		document: document.clone(),
		placement,
		declared_in,
		created: false,
		extended: true,
		new_patterns,
	})
}

/// Whether `existing` and `added` declare the same source of `document`:
/// paths that resolve to the same file, or patterns written alike.
fn same_source(
	document: &DocumentId,
	existing: &SourceDecl,
	added: &SourceDecl,
) -> bool {
	let resolve = |declared: &str| match document {
		DocumentId::Local(path) => resolve_source(path, declared).ok(),
		DocumentId::Remote(_) => resolve_rooted(declared).ok(),
	};
	match (existing, added) {
		(SourceDecl::Path(existing), SourceDecl::Path(added)) => {
			resolve(&existing.path).is_some()
				&& resolve(&existing.path) == resolve(&added.path)
		}
		(SourceDecl::Pattern(existing), SourceDecl::Pattern(added)) => {
			existing.pattern == added.pattern
		}
		_ => false,
	}
}

/// Confirms a registration against the discovery of the result, and refuses
/// a new pattern that matches nothing.
///
/// # Errors
///
/// Fails with [`MutateError::EmptyPattern`], or with
/// [`MutateError::RoundTrip`] when discovery does not find the declaration
/// where it was planned.
pub(super) fn confirm(
	after: &Inventory,
	pending: Pending,
) -> Result<Registered, MutateError> {
	let claim = rediscovered(
		after,
		&pending.document,
		&pending.placement,
		&pending.declared_in,
	)?;
	let mut matches = Vec::new();
	for source in &claim.sources {
		if let ResolvedSource::Pattern(pattern) = source {
			let found: Vec<RepoPath> =
				pattern.matches.iter().flatten().cloned().collect();
			let declared = declared(claim, pattern.index);
			if found.is_empty() && pending.new_patterns.contains(&declared) {
				return Err(MutateError::EmptyPattern {
					document: pending.document,
					pattern: declared,
				});
			}
			matches.push(PatternMatches {
				pattern: declared,
				matches: found,
			});
		}
	}
	Ok(Registered {
		document: pending.document,
		placement: pending.placement,
		declared_in: pending.declared_in,
		created: pending.created,
		extended: pending.extended,
		declaration: claim.declaration.clone(),
		matches,
	})
}

/// The one claim of `document` after the change, which must be at
/// `placement`.
///
/// # Errors
///
/// Fails with [`MutateError::RoundTrip`] otherwise.
pub(super) fn rediscovered<'a>(
	after: &'a Inventory,
	document: &DocumentId,
	placement: &Placement,
	declared_in: &Location,
) -> Result<&'a Claim, MutateError> {
	let claims: Vec<&'a Claim> = after
		.claims
		.iter()
		.filter(|claim| &claim.document == document)
		.collect();
	match claims.as_slice() {
		[claim] if Placement::of(claim) == *placement => Ok(claim),
		_ => Err(MutateError::RoundTrip {
			path: declared_in.file.clone(),
		}),
	}
}

/// Checks that `path` can be registered: a tracked document that is not one
/// of Source Sentry's own files, and that nothing claims.
///
/// # Errors
///
/// Fails with [`MutateError::NotADocument`],
/// [`MutateError::DocumentNotFound`], or as [`unclaimed`] does.
fn unclaimed_local<S: Snapshot>(
	repo: &Repo<'_, S>,
	path: &RepoPath,
) -> Result<(), MutateError> {
	if is_own_file(path) {
		return Err(MutateError::NotADocument { path: path.clone() });
	}
	if !repo.tracked.contains(path) {
		return Err(MutateError::DocumentNotFound { path: path.clone() });
	}
	unclaimed(repo, &DocumentId::Local(path.clone()))
}

/// Checks that nothing claims `document`, and that discovery reports no
/// configuration or integrity error about it.
///
/// # Errors
///
/// Fails with [`MutateError::AlreadyClaimed`] or
/// [`MutateError::DocumentHasErrors`].
fn unclaimed<S: Snapshot>(
	repo: &Repo<'_, S>,
	document: &DocumentId,
) -> Result<(), MutateError> {
	let sites: Vec<Location> = repo
		.inventory
		.claims_of(document)
		.map(Claim::location)
		.collect();
	if !sites.is_empty() {
		return Err(MutateError::AlreadyClaimed {
			document: document.clone(),
			sites,
		});
	}
	repo.refuse_errors(document, &[])
}

/// Checks that `path` may carry an on-file declaration: its adapter is
/// `md-utf8`, and no other document uses it as a source.
///
/// # Errors
///
/// Fails with [`MutateError::NotDeclarationCapable`] or
/// [`MutateError::UsedAsSource`].
pub(super) fn on_file_allowed<S: Snapshot>(
	repo: &Repo<'_, S>,
	configs: &ConfigSet,
	path: &RepoPath,
) -> Result<(), MutateError> {
	let adapter = configs.adapter(path);
	if !adapter.is_some_and(Adapter::carries_declaration) {
		return Err(MutateError::NotDeclarationCapable {
			document: DocumentId::Local(path.clone()),
			adapter,
		});
	}
	let dependents = repo.dependents_of(path);
	if dependents.is_empty() {
		Ok(())
	} else {
		Err(MutateError::UsedAsSource {
			document: DocumentId::Local(path.clone()),
			dependents,
		})
	}
}

/// The directory of the local lockfile that will list `document`: `requested`,
/// which must contain the document, or else the nearest existing lockfile, or
/// else the document's own directory.
///
/// Only the nearest lockfile may list a document, so every lockfile between
/// the document and `requested` must be one that this change empties and
/// therefore deletes: at least one of its entries is among
/// `moving`, and all of them are. This is how a nested lockfile merges back
/// into its parent.
///
/// # Errors
///
/// Fails with [`MutateError::NotAnAncestor`]; with
/// [`MutateError::NearerLockfile`] for a lockfile in between that none of
/// `moving` leaves; and with [`MutateError::StrandedEntries`] for one that
/// would keep entries outside `moving`.
pub(super) fn lockfile_directory<S: Snapshot>(
	repo: &Repo<'_, S>,
	document: &RepoPath,
	requested: Option<&RepoPath>,
	moving: &BTreeSet<DocumentId>,
) -> Result<RepoPath, MutateError> {
	let nearest = repo.nearest_lockfile(document);
	let Some(directory) = requested else {
		return Ok(
			nearest.unwrap_or_else(|| document.parent().unwrap_or(RepoPath::ROOT)),
		);
	};
	if directory == document || !document.starts_with(directory) {
		return Err(MutateError::NotAnAncestor {
			document: document.clone(),
			directory: directory.clone(),
		});
	}
	for between in repo.lockfiles_between(document, directory) {
		let lockfile = lockfile_path(&between);
		let (moved, kept): (Vec<DocumentId>, Vec<DocumentId>) = repo
			.entries_of(&lockfile)
			.into_iter()
			.partition(|entry| moving.contains(entry));
		if moved.is_empty() {
			return Err(MutateError::NearerLockfile {
				document: document.clone(),
				directory: directory.clone(),
				nearest: lockfile,
			});
		}
		if !kept.is_empty() {
			return Err(MutateError::StrandedEntries {
				lockfile,
				directory: between,
				documents: kept,
			});
		}
	}
	Ok(directory.clone())
}

/// Prepares the lockfile of `directory` to receive `document`, after
/// [`lockfile_directory`] chose it, and returns whether the plan creates it.
/// Authority moves between lockfiles in two cases:
///
/// - **Merging.** Each lockfile between `document` and `directory` is
///   emptied by the change and deleted, so its tombstones move
///   into the lockfile of `directory`.
/// - **Splitting.** A new lockfile takes authority over its directory's
///   subtree from the lockfile that held it. Every entry there must be among
///   `moving`, and every tombstone there moves into the new lockfile.
///
/// Each tombstone moved is recorded in `carried`.
///
/// # Errors
///
/// Fails with [`MutateError::StrandedEntries`] for entries outside
/// `moving`, with [`MutateError::TombstoneConflict`], and as
/// [`Edits::local`] does.
pub(super) fn prepare_lockfile<S: Snapshot>(
	repo: &Repo<'_, S>,
	edits: &mut Edits,
	document: &RepoPath,
	directory: &RepoPath,
	moving: &BTreeSet<DocumentId>,
	carried: &mut Vec<CarriedTombstone>,
) -> Result<bool, MutateError> {
	for between in repo.lockfiles_between(document, directory) {
		carry_tombstones(repo, edits, &between, directory, &between, carried)?;
	}
	let path = lockfile_path(directory);
	if repo.tracked.contains(&path) {
		return Ok(false);
	}
	let Some(parent) = repo.nearest_lockfile(&path) else {
		return Ok(true);
	};
	let parent_path = lockfile_path(&parent);
	let below = |document: &DocumentId| match document {
		DocumentId::Local(document) => document.starts_with(directory),
		DocumentId::Remote(_) => false,
	};
	let stranded: Vec<DocumentId> = repo
		.inventory
		.claims
		.iter()
		.filter(|claim| {
			claim.mode == Mode::LocalLockfile
				&& claim.declared_in == parent_path
				&& below(&claim.document)
				&& !moving.contains(&claim.document)
		})
		.map(|claim| claim.document.clone())
		.collect();
	if !stranded.is_empty() {
		return Err(MutateError::StrandedEntries {
			lockfile: parent_path,
			directory: directory.clone(),
			documents: stranded,
		});
	}
	carry_tombstones(repo, edits, &parent, directory, directory, carried)?;
	Ok(true)
}

/// Moves every tombstone below `below` from the lockfile of `from` to the
/// lockfile of `to`, recording each in `carried`. A tombstone that this
/// change has moved already is skipped.
///
/// # Errors
///
/// Fails with [`MutateError::TombstoneConflict`] when the lockfile of `to`
/// already holds a tombstone for the same document, and as [`Edits::local`]
/// does.
fn carry_tombstones<S: Snapshot>(
	repo: &Repo<'_, S>,
	edits: &mut Edits,
	from: &RepoPath,
	to: &RepoPath,
	below: &RepoPath,
	carried: &mut Vec<CarriedTombstone>,
) -> Result<(), MutateError> {
	let from_path = lockfile_path(from);
	let to_path = lockfile_path(to);
	let retired: Vec<RepoPath> = repo
		.inventory
		.tombstones
		.iter()
		.filter(|tombstone| tombstone.lockfile == from_path)
		.filter_map(|tombstone| match &tombstone.document {
			DocumentId::Local(document) if document.starts_with(below) => {
				Some(document.clone())
			}
			DocumentId::Local(_) | DocumentId::Remote(_) => None,
		})
		.collect();
	for document in retired {
		let Some(tombstone) = edits.local(repo, from)?.retired.remove(&document)
		else {
			continue;
		};
		let target = edits.local(repo, to)?;
		if target.retired.contains_key(&document) {
			return Err(MutateError::TombstoneConflict {
				document: DocumentId::Local(document),
				from: from_path,
				to: to_path,
			});
		}
		target.retired.insert(document.clone(), tombstone);
		carried.push(CarriedTombstone {
			document: DocumentId::Local(document),
			from: from_path.clone(),
			to: to_path.clone(),
		});
	}
	Ok(())
}

/// The file and field where `document`'s declaration lives at `placement`.
///
/// # Errors
///
/// Fails with [`MutateError::TargetMismatch`] when the placement does not
/// fit the document's kind, and with [`MutateError::InvalidOwnerName`].
pub(super) fn location(
	document: &DocumentId,
	placement: &Placement,
) -> Result<Location, MutateError> {
	match (document, placement) {
		(DocumentId::Local(path), Placement::OnFile) => {
			Ok(Location::field(path.clone(), FRONTMATTER_FIELD))
		}
		(DocumentId::Local(path), Placement::Lockfile { directory }) => {
			let key = path.strip_prefix(directory).unwrap_or(path.as_str());
			Ok(Location::field(
				lockfile_path(directory),
				entry_field("documents", key),
			))
		}
		(DocumentId::Remote(identity), Placement::Remote { owner }) => {
			Ok(Location::field(
				remote_lockfile_path(owner)?,
				entry_field("documents", &identity.to_string()),
			))
		}
		(document, placement) => Err(MutateError::TargetMismatch {
			document: document.clone(),
			target: placement.to_string(),
		}),
	}
}

/// The unattested declaration that `request` describes.
///
/// # Errors
///
/// Fails with [`MutateError::UndeclaredClass`], [`MutateError::EmptyOwner`],
/// [`MutateError::InvalidSchedule`], [`MutateError::UrlNotRemote`], and as
/// [`sources`] does.
fn declaration<S: Snapshot>(
	repo: &Repo<'_, S>,
	configs: &ConfigSet,
	request: &TrackRequest,
) -> Result<Declaration, MutateError> {
	if let Some(class) = request
		.classes
		.iter()
		.find(|class| !configs.classes().contains_key(*class))
	{
		return Err(MutateError::UndeclaredClass {
			class: class.clone(),
		});
	}
	if request.owners.iter().any(|owner| owner.trim().is_empty()) {
		return Err(MutateError::EmptyOwner);
	}
	if let Some(schedule) = &request.schedule {
		EffectiveSchedule::new(schedule)?;
	}
	if request.url.is_some() && !matches!(request.document, DocumentId::Remote(_))
	{
		return Err(MutateError::UrlNotRemote {
			document: request.document.clone(),
		});
	}
	Ok(Declaration {
		classes: request.classes.clone(),
		owners: request.owners.clone(),
		schedule: request.schedule.clone(),
		sources: sources(repo, &request.document, &request.sources)?,
		url: request.url.clone(),
		..Declaration::default()
	})
}

/// The unattested sources that `specs` describe for `document`.
///
/// # Errors
///
/// Fails with [`MutateError::InvalidSource`] for a path that does not
/// resolve (relative to the document, or to the repository root with a
/// leading `/`), or a remote document's source that is not `/`-rooted; with
/// [`MutateError::MissingSource`] for a path that is not tracked; with
/// [`MutateError::SelfDependency`]; with [`MutateError::DuplicateSource`]; and
/// with [`MutateError::EmptyPattern`] for a pattern that matches no tracked
/// file; and with [`MutateError::SubmoduleSource`] for a path, or a new
/// pattern's match, that is a submodule. A pattern that does not compile is
/// left to discovery, whose diagnostic names the problem.
fn sources<S: Snapshot>(
	repo: &Repo<'_, S>,
	document: &DocumentId,
	specs: &[SourceSpec],
) -> Result<Vec<SourceDecl>, MutateError> {
	let local = match document {
		DocumentId::Local(path) => Some(path),
		DocumentId::Remote(_) => None,
	};
	let invalid = |declared: &str, error| MutateError::InvalidSource {
		document: document.clone(),
		declared: declared.to_owned(),
		error,
	};
	let mut paths: BTreeMap<RepoPath, &str> = BTreeMap::new();
	let mut patterns: BTreeSet<&str> = BTreeSet::new();
	let mut declared = Vec::with_capacity(specs.len());
	// Every file a new source names, with the source as declared, so that a
	// submodule is refused here rather than reported by every later `check`.
	let mut named: Vec<(RepoPath, &str)> = Vec::new();
	for spec in specs {
		match spec {
			SourceSpec::Path(source) => {
				let path = match local {
					Some(document) => resolve_source(document, source),
					None => resolve_rooted(source),
				}
				.map_err(|error| invalid(source, error))?;
				if local == Some(&path) {
					return Err(MutateError::SelfDependency {
						document: document.clone(),
						declared: source.clone(),
					});
				}
				if !repo.tracked.contains(&path) {
					return Err(MutateError::MissingSource {
						document: document.clone(),
						declared: source.clone(),
						path,
					});
				}
				named.push((path.clone(), source));
				if let Some(first) = paths.insert(path, source) {
					return Err(MutateError::DuplicateSource {
						document: document.clone(),
						first: first.to_owned(),
						declared: source.clone(),
						pattern: false,
					});
				}
				declared.push(SourceDecl::Path(PathSource {
					oid: None,
					path: source.clone(),
					reviewed: None,
				}));
			}
			SourceSpec::Pattern(pattern) => {
				if pattern.is_empty() {
					return Err(invalid(pattern, PathError::Empty));
				}
				if local.is_none() && !pattern.starts_with('/') {
					return Err(invalid(pattern, PathError::NotRooted(pattern.clone())));
				}
				if !patterns.insert(pattern) {
					return Err(MutateError::DuplicateSource {
						document: document.clone(),
						first: pattern.clone(),
						declared: pattern.clone(),
						pattern: true,
					});
				}
				if let Ok(glob) = SourceGlob::compile(local, pattern) {
					let matches = glob.expand(&repo.tracked);
					if matches.is_empty() {
						return Err(MutateError::EmptyPattern {
							document: document.clone(),
							pattern: pattern.clone(),
						});
					}
					named
						.extend(matches.into_iter().map(|path| (path, pattern.as_str())));
				}
				declared.push(SourceDecl::Pattern(PatternSource {
					members: Vec::new(),
					pattern: pattern.clone(),
				}));
			}
		}
	}
	let files: Vec<RepoPath> =
		named.iter().map(|(path, _)| path.clone()).collect();
	let canonical = repo.canonical(&files)?;
	if let Some(((path, source), _)) = named
		.into_iter()
		.zip(canonical)
		.find(|(_, canonical)| *canonical == Canonical::Submodule)
	{
		return Err(MutateError::SubmoduleSource {
			document: document.clone(),
			declared: source.to_owned(),
			path,
		});
	}
	Ok(declared)
}
