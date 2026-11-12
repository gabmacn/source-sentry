//! `attest`: recording that a person reviewed a document against its
//! sources.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};

use super::expectation::{Expectation, compare};
use super::repo::{Repo, checked_reviewer};
use super::select::{Observed, SourceState, observe};
use super::verify::verify;
use super::{MutateError, Planned, Selection, edits::Edits};
use crate::claims::{
	Attestation, Claim, DiagnosticKind, Inventory, Location, ResolvedSource,
};
use crate::model::{
	Declaration, DocumentId, Lifecycle, Member, ObjectId, RepoPath, Review,
	SourceDecl, resolve_source,
};
use crate::ports::{Canonical, Snapshot};

/// What to attest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttestRequest {
	/// The document.
	pub document: DocumentId,
	/// The sources to attest.
	pub selection: Selection,
	/// The expectations the reviewer approved, usually from `diff`. When
	/// given, the snapshot must match them exactly for the selection,
	/// pattern membership included. When `None`, the current state is
	/// recorded.
	pub expectations: Option<Vec<Expectation>>,
	/// The reviewer's name: `--reviewer`, or else git's `user.name`, which
	/// the CLI passes in. `None` records an unknown reviewer.
	pub reviewer: Option<String>,
	/// The instant of the review, which is truncated to the second.
	pub at: DateTime<Utc>,
	/// Whether the whole document was reread, which alone updates the
	/// document's own `reviewed`.
	pub reread: bool,
}

/// What an attestation records.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttestSummary {
	/// The document.
	pub document: DocumentId,
	/// Where its declaration is written.
	pub declared_in: Location,
	/// Each attested source, in declared order.
	pub sources: Vec<AttestedSource>,
	/// The expectations the attestation binds: every file of the selection
	/// and every acknowledged removal, sorted by path.
	pub expectations: Vec<Expectation>,
	/// The review recorded on each attested source.
	pub review: Review,
	/// Whether the document's own `reviewed` was updated.
	pub reread: bool,
	/// The completion recorded for a `living` series entry, when this
	/// attestation completes it.
	pub completed: Option<Review>,
}

/// One attested source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttestedSource {
	/// The path or pattern, as declared.
	pub declared: String,
	/// Whether it is a pattern.
	pub pattern: bool,
	/// Its files: the path of a path source, or every current and former
	/// member of a pattern, sorted by path.
	pub entries: Vec<AttestedEntry>,
}

/// One file of an attested source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttestedEntry {
	/// The file.
	pub path: RepoPath,
	/// The oid attested before, or `None` when it was never attested.
	pub previous: Option<ObjectId>,
	/// The oid recorded now, or `None` for a member that left its pattern
	/// and is dropped.
	pub recorded: Option<ObjectId>,
}

/// The errors about the document that `attest` handles itself: a missing
/// source refuses only when it is selected.
const TOLERATED: &[DiagnosticKind] = &[DiagnosticKind::MissingSource];

/// Plans an attestation.
///
/// For each selected path source, records its current canonical oid. For
/// each selected pattern, replaces its members with the files it matches now
/// and their oids, so additions are recorded and removals dropped. Every
/// attested source, including a pattern with no remaining members, and each
/// current member gets `Review::new(at, reviewer)`. Unselected sources keep
/// their reviews. Only `reread` updates the document's own `reviewed`.
/// When the document is an entry of a `living` series, has no `completed`
/// yet, and every source is attested at
/// its current oid once this attestation is recorded, `completed` is set to
/// the same review.
///
/// An on-file declaration is rewritten in the document's frontmatter, and a
/// lockfile entry in its lockfile.
///
/// # Errors
///
/// Refuses, planning nothing:
///
/// - when the document is untracked, claimed twice, or has configuration or
///   integrity errors other than a missing source that is not selected;
/// - for a sealed record ([`MutateError::Sealed`]), since `attest` never
///   seals or reseals;
/// - for an invalid reviewer name;
/// - when the selection names no source or an unknown one, or includes a
///   missing path source;
/// - for a selected pattern with no review or attested members that matches
///   nothing ([`MutateError::EmptyPattern`]);
/// - for a file without a canonical oid;
/// - when the selection is empty and `reread` is not set
///   ([`MutateError::NothingToAttest`]);
/// - when expectations are given and the snapshot differs from them in any
///   way ([`MutateError::ExpectationMismatch`]);
/// - when the result would not be valid, as [`super`] describes, and when
///   the backend fails.
pub fn attest<S: Snapshot>(
	snapshot: &S,
	inventory: &Inventory,
	request: &AttestRequest,
) -> Result<Planned<AttestSummary>, MutateError> {
	let repo = Repo::new(snapshot, inventory)?;
	let reviewer = checked_reviewer(request.reviewer.as_deref())?;
	let claim = repo.claim(&request.document)?;
	if claim.is_sealed() {
		return Err(MutateError::Sealed {
			document: claim.document.clone(),
		});
	}
	repo.refuse_errors(&claim.document, TOLERATED)?;
	let observed = observe(&repo, claim, &request.selection)?;
	refuse_new_empty_patterns(claim, &observed)?;
	if observed.sources.is_empty() && !request.reread {
		return Err(MutateError::NothingToAttest {
			document: claim.document.clone(),
		});
	}
	let expectations = observed.expectations();
	if let Some(given) = &request.expectations {
		let mismatches = compare(&expectations, given);
		if !mismatches.is_empty() {
			return Err(MutateError::ExpectationMismatch {
				document: claim.document.clone(),
				mismatches,
			});
		}
	}

	let review = Review::new(request.at, reviewer);
	let mut declaration = claim.declaration.clone();
	let sources = record(
		&inventory.project,
		claim,
		&observed,
		&review,
		&mut declaration,
	);
	if request.reread {
		declaration.reviewed = Some(review.clone());
	}
	let completed = completes(&repo, claim, &observed)?.then(|| review.clone());
	if let Some(completed) = &completed {
		declaration.completed = Some(completed.clone());
	}

	let mut edits = Edits::default();
	edits.set(&repo, claim, Some(declaration))?;
	let plan = edits.finish(&repo)?;
	verify(&repo, &plan)?;
	Ok(Planned {
		plan,
		summary: AttestSummary {
			document: claim.document.clone(),
			declared_in: claim.location(),
			sources,
			expectations,
			review,
			reread: request.reread,
			completed,
		},
	})
}

/// Refuses a selected pattern that was never attested and matches nothing.
/// A pattern whose members have all gone is an ordinary change.
fn refuse_new_empty_patterns(
	claim: &Claim,
	observed: &Observed,
) -> Result<(), MutateError> {
	for source in &observed.sources {
		if let SourceState::Pattern { matches, members } = &source.state
			&& matches.is_empty()
			&& members.is_empty()
			&& !matches!(
				claim.declaration.sources.get(source.index),
				Some(SourceDecl::Pattern(pattern)) if pattern.reviewed.is_some()
			) {
			return Err(MutateError::EmptyPattern {
				document: claim.document.clone(),
				pattern: source.declared.clone(),
			});
		}
	}
	Ok(())
}

/// Records the observed state of each selected source in `declaration`, and
/// describes what changed.
fn record(
	project: &RepoPath,
	claim: &Claim,
	observed: &Observed,
	review: &Review,
	declaration: &mut Declaration,
) -> Vec<AttestedSource> {
	let document = match &claim.document {
		DocumentId::Local(path) => Some(path),
		DocumentId::Remote(_) => None,
	};
	observed
		.sources
		.iter()
		.filter_map(|source| {
			let entries =
				match (&source.state, declaration.sources.get_mut(source.index)?) {
					(
						SourceState::Path {
							path,
							attestation,
							current,
						},
						SourceDecl::Path(declared),
					) => {
						declared.oid = Some(current.clone());
						declared.reviewed = Some(review.clone());
						vec![AttestedEntry {
							path: path.clone(),
							previous: attestation.as_ref().map(|old| old.oid.clone()),
							recorded: Some(current.clone()),
						}]
					}
					(
						SourceState::Pattern { matches, members },
						SourceDecl::Pattern(declared),
					) => {
						declared.reviewed = Some(review.clone());
						declared.members = matches
							.iter()
							.map(|(path, oid)| Member {
								oid: oid.clone(),
								path: member_spelling(
									project,
									document,
									&declared.pattern,
									path,
								),
								reviewed: review.clone(),
							})
							.collect();
						pattern_entries(matches, members)
					}
					// Discovery resolves each declared source as what it is
					// declared as, so the kinds always agree.
					(SourceState::Path { .. }, SourceDecl::Pattern(_))
					| (SourceState::Pattern { .. }, SourceDecl::Path(_)) => return None,
				};
			Some(AttestedSource {
				declared: source.declared.clone(),
				pattern: matches!(source.state, SourceState::Pattern { .. }),
				entries,
			})
		})
		.collect()
}

/// Every current and former member of a pattern, with its old and new oid.
fn pattern_entries(
	matches: &BTreeMap<RepoPath, ObjectId>,
	members: &BTreeMap<RepoPath, Attestation>,
) -> Vec<AttestedEntry> {
	let paths: BTreeSet<&RepoPath> =
		matches.keys().chain(members.keys()).collect();
	paths
		.into_iter()
		.map(|path| AttestedEntry {
			path: path.clone(),
			previous: members.get(path).map(|old| old.oid.clone()),
			recorded: matches.get(path).cloned(),
		})
		.collect()
}

/// How a member of `pattern` is written.
///
/// A member of a `/`-rooted pattern, or of a remote document's pattern, is
/// written from the repository root, such as `/db/0001.sql`. A member of a
/// relative pattern is written relative to the document, climbing with `../`
/// where needed, so that it reads like the pattern it belongs to.
fn member_spelling(
	project: &RepoPath,
	document: Option<&RepoPath>,
	pattern: &str,
	member: &RepoPath,
) -> String {
	let rooted = if pattern.starts_with(":/") {
		format!(":/{}", member.as_str())
	} else if let Some(relative) = member.strip_prefix(project) {
		format!("/{relative}")
	} else {
		format!(":/{}", member.as_str())
	};
	let Some(document) = document else {
		return rooted;
	};
	if pattern.starts_with('/') || pattern.starts_with(":/") {
		return rooted;
	}
	let directory = document.parent().unwrap_or(RepoPath::ROOT);
	let base: Vec<&str> = directory.components().collect();
	let target: Vec<&str> = member.components().collect();
	let shared = base
		.iter()
		.zip(&target)
		.take_while(|(left, right)| left == right)
		.count();
	let mut parts: Vec<&str> = vec![".."; base.len() - shared];
	parts.extend(&target[shared..]);
	let relative = parts.join("/");
	if resolve_source(document, &relative).is_ok_and(|path| &path == member) {
		relative
	} else {
		rooted
	}
}

/// Whether this attestation completes a `living` series entry: the document
/// has no `completed` yet, is a member of a `living` series, and once the
/// selected sources are recorded, every source is attested at its current
/// oid, every current match of a pattern included and no former member
/// outstanding.
///
/// # Errors
///
/// Fails when the backend fails.
fn completes<S: Snapshot>(
	repo: &Repo<'_, S>,
	claim: &Claim,
	observed: &Observed,
) -> Result<bool, MutateError> {
	if claim.declaration.completed.is_some() {
		return Ok(false);
	}
	let DocumentId::Local(path) = &claim.document else {
		return Ok(false);
	};
	let living = repo
		.inventory
		.series_of(path)
		.is_some_and(|(series, _)| series.series.lifecycle == Lifecycle::Living);
	// A source that discovery could not resolve can never count as attested.
	if !living || claim.sources.len() != claim.declaration.sources.len() {
		return Ok(false);
	}
	let selected: BTreeSet<usize> =
		observed.sources.iter().map(|source| source.index).collect();
	let rest: Vec<&ResolvedSource> = claim
		.sources
		.iter()
		.filter(|source| !selected.contains(&source.index()))
		.collect();
	let mut files = BTreeSet::new();
	for source in &rest {
		match source {
			ResolvedSource::Path(source) if source.exists => {
				files.insert(source.path.clone());
			}
			ResolvedSource::Pattern(source) => {
				files.extend(source.matches.iter().flatten().cloned());
			}
			ResolvedSource::Path(_) => return Ok(false),
		}
	}
	let files: Vec<RepoPath> = files.into_iter().collect();
	let canonical = repo.canonical(&files)?;
	let current: BTreeMap<RepoPath, ObjectId> = files
		.into_iter()
		.zip(canonical)
		.filter_map(|(path, canonical)| match canonical {
			Canonical::Oid(oid) => Some((path, oid)),
			_ => None,
		})
		.collect();
	Ok(rest.iter().all(|source| is_fresh(source, &current)))
}

/// Whether an unselected source is attested at its current oid.
fn is_fresh(
	source: &ResolvedSource,
	current: &BTreeMap<RepoPath, ObjectId>,
) -> bool {
	let attested = |path: &RepoPath, attestation: Option<&Attestation>| {
		attestation
			.is_some_and(|attestation| current.get(path) == Some(&attestation.oid))
	};
	match source {
		ResolvedSource::Path(source) => {
			attested(&source.path, source.attestation.as_ref())
		}
		ResolvedSource::Pattern(source) => {
			source.matches.as_ref().is_some_and(|matches| {
				matches.iter().eq(source.members.keys())
					&& matches
						.iter()
						.all(|path| attested(path, source.members.get(path)))
			})
		}
	}
}
