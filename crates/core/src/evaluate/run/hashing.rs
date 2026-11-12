//! Canonical oids for every live source and every sealed record, in one batch.
//!
//! A path that cannot be canonicalized is reported once, and left out of the
//! facts of every document that names it:
//!
//! - a submodule is a configuration error, reported at each
//!   declaration that names it;
//! - an unsupported filter, an LFS pointer with extensions, or CRLF already
//!   in the index is an operational error (exit 4): the conversion is
//!   unsupported, so the evaluation is incomplete.

use std::collections::BTreeMap;

use crate::claims::{
	Claim, Diagnostic, DiagnosticKind, Inventory, Location, Presence,
	ResolvedSource,
};
use crate::model::DocumentId;
use crate::ports::{
	BackendError, BackendErrorKind, Canonical, ObjectId, RepoPath, Snapshot,
};

/// The canonical oid of every path that evaluation compares.
#[derive(Debug, Default)]
pub(super) struct Hashes {
	canonical: BTreeMap<RepoPath, Canonical>,
}

impl Hashes {
	/// The canonical oid of `path`, or `None` when it was not hashed or
	/// could not be canonicalized.
	pub(super) fn oid(&self, path: &RepoPath) -> Option<&ObjectId> {
		match self.canonical.get(path) {
			Some(Canonical::Oid(oid)) => Some(oid),
			Some(
				Canonical::UnsupportedFilter { .. }
				| Canonical::UnsupportedLfsExtension
				| Canonical::CrlfInIndex
				| Canonical::Submodule,
			)
			| None => None,
		}
	}
}

/// A declaration that names a hashed path, for diagnostics.
struct Naming {
	location: Location,
	document: DocumentId,
}

/// Hashes every current source of every live claim (each path source that
/// exists, and each current match of each pattern) and every present sealed
/// record, deduplicated, with one [`Snapshot::canonical_oids`] call.
///
/// A sealed record's sources are frozen, so they are not hashed.
/// A missing explicit source has a discovery diagnostic already, so it is
/// not hashed either.
///
/// # Errors
///
/// Fails when the backend fails, or returns a different number of values
/// than paths.
pub(super) fn hash<S: Snapshot>(
	snapshot: &S,
	inventory: &Inventory,
	diagnostics: &mut Vec<Diagnostic>,
) -> Result<Hashes, BackendError> {
	let mut wanted: BTreeMap<RepoPath, Vec<Naming>> = BTreeMap::new();
	for claim in &inventory.claims {
		collect(claim, &mut wanted);
	}
	if wanted.is_empty() {
		return Ok(Hashes::default());
	}
	let paths: Vec<RepoPath> = wanted.keys().cloned().collect();
	let values = snapshot.canonical_oids(&paths)?;
	if values.len() != paths.len() {
		return Err(BackendError::new(
			BackendErrorKind::Git,
			format!(
				"the backend returned {} canonical oids for {} paths",
				values.len(),
				paths.len()
			),
		));
	}
	let mut canonical = BTreeMap::new();
	for ((path, namings), value) in wanted.into_iter().zip(values) {
		diagnose(&path, &value, &namings, diagnostics);
		canonical.insert(path, value);
	}
	Ok(Hashes { canonical })
}

/// Adds the paths `claim` needs hashed, with where it names each.
fn collect(claim: &Claim, wanted: &mut BTreeMap<RepoPath, Vec<Naming>>) {
	let mut add = |path: &RepoPath, field: String| {
		let location = claim.location_of(field);
		wanted.entry(path.clone()).or_default().push(Naming {
			location,
			document: claim.document.clone(),
		});
	};
	if claim.is_sealed() {
		if let (DocumentId::Local(path), Presence::Present) =
			(&claim.document, claim.presence)
		{
			add(path, format!("{}.seal", claim.field));
		}
		return;
	}
	for source in &claim.sources {
		match source {
			ResolvedSource::Path(source) if source.exists => {
				add(
					&source.path,
					format!("{}.sources[{}].path", claim.field, source.index),
				);
			}
			ResolvedSource::Path(_) => {}
			ResolvedSource::Pattern(source) => {
				for path in source.matches.iter().flatten() {
					add(
						path,
						format!("{}.sources[{}].pattern", claim.field, source.index),
					);
				}
			}
		}
	}
}

/// Reports a path that could not be canonicalized.
fn diagnose(
	path: &RepoPath,
	value: &Canonical,
	namings: &[Naming],
	diagnostics: &mut Vec<Diagnostic>,
) {
	const NO_GUESS: &str = "no oid was guessed";
	let (kind, message) = match value {
		Canonical::Oid(_) => return,
		Canonical::Submodule => {
			// The diagnostic is reported at each declaration that names it.
			for naming in namings {
				diagnostics.push(
					Diagnostic::new(
						DiagnosticKind::Submodule,
						naming.location.clone(),
						format!(
							"`{path}` is a submodule, which Source Sentry does not support. \
							 Remove it from the sources, or narrow the pattern to leave it \
							 out"
						),
					)
					.with_related(vec![Location::file(path.clone())])
					.about(naming.document.clone()),
				);
			}
			return;
		}
		Canonical::UnsupportedFilter {
			filter,
			attribute_origin,
		} => (
			DiagnosticKind::UnsupportedFilter,
			format!(
				"Cannot canonicalize {path}: unsupported clean filter {filter} \
				 ({attribute_origin}); {NO_GUESS}. Evaluate committed blobs with \
				 --rev, or use a supported conversion"
			),
		),
		Canonical::UnsupportedLfsExtension => (
			DiagnosticKind::UnsupportedLfsExtension,
			format!(
				"Cannot canonicalize {path}: its Git LFS pointer uses extensions, \
				 which Source Sentry does not support; {NO_GUESS}. Evaluate committed \
				 blobs with --rev, or store the file without LFS extensions"
			),
		),
		Canonical::CrlfInIndex => (
			DiagnosticKind::CrlfInIndex,
			format!(
				"Cannot canonicalize {path}: the index holds it with CRLF line \
				 endings although its attributes ask git to normalize them, so `git \
				 hash-object` and `git add` disagree; {NO_GUESS}. Run `git add \
				 --renormalize .` and commit the result"
			),
		),
	};
	let related = namings
		.iter()
		.map(|naming| naming.location.clone())
		.collect();
	diagnostics.push(
		Diagnostic::new(kind, Location::file(path.clone()), message)
			.with_related(related),
	);
}
