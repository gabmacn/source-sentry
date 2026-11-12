//! Selecting a document's sources, and observing their current state.
//!
//! `attest` and `diff` share this, so the expectations `diff` emits are
//! exactly those `attest` compares.

use std::collections::{BTreeMap, BTreeSet};

use super::MutateError;
use super::SourceSpec;
use super::expectation::{self, Expectation};
use super::repo::Repo;
use crate::claims::{Attestation, Claim, ResolvedSource};
use crate::model::{ObjectId, RepoPath, SourceDecl, resolve_rooted};
use crate::ports::Snapshot;

/// Which sources of a document a command covers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Selection {
	/// Every declared source.
	#[default]
	All,
	/// The sources named here. Each name is a source's path or pattern
	/// exactly as declared, or, written with a leading `/`, its repository
	/// path or its pattern resolved against the repository root.
	Sources(Vec<String>),
	/// Sources identified by their exact declared spelling and kind, after
	/// a caller has resolved its own selection syntax without ambiguity.
	Declared(Vec<SourceSpec>),
}

/// The selected sources of a claim, with the current canonical oid of every
/// file they include.
pub(super) struct Observed {
	/// The selected sources, in declared order.
	pub(super) sources: Vec<ObservedSource>,
}

/// One selected source.
pub(super) struct ObservedSource {
	/// Its index in the declaration's sources.
	pub(super) index: usize,
	/// The path or pattern, as declared.
	pub(super) declared: String,
	/// What it includes now, and what was attested.
	pub(super) state: SourceState,
}

/// The state of one selected source.
pub(super) enum SourceState {
	/// A path source, which exists.
	Path {
		/// The canonical path.
		path: RepoPath,
		/// The attestation, if any.
		attestation: Option<Attestation>,
		/// The current canonical oid.
		current: ObjectId,
	},
	/// A pattern source.
	Pattern {
		/// Every current match, with its canonical oid.
		matches: BTreeMap<RepoPath, ObjectId>,
		/// The attested members.
		members: BTreeMap<RepoPath, Attestation>,
	},
}

impl Observed {
	/// The expectations that describe the selection: each current file, and
	/// each attested member that has left its pattern and that no other
	/// selected source includes.
	pub(super) fn expectations(&self) -> Vec<Expectation> {
		let mut present = BTreeMap::new();
		let mut removed = BTreeSet::new();
		for source in &self.sources {
			match &source.state {
				SourceState::Path { path, current, .. } => {
					present.insert(path.clone(), current.clone());
				}
				SourceState::Pattern { matches, members } => {
					present.extend(
						matches
							.iter()
							.map(|(path, oid)| (path.clone(), oid.clone())),
					);
					removed.extend(
						members
							.keys()
							.filter(|path| !matches.contains_key(*path))
							.cloned(),
					);
				}
			}
		}
		expectation::describe(&present, &removed)
	}
}

/// The sources of `claim` that `selection` names, in declared order.
///
/// # Errors
///
/// Fails with [`MutateError::NothingSelected`] for an empty list, and with
/// [`MutateError::UnknownSource`] for a name that matches no source.
pub(super) fn select<'c>(
	claim: &'c Claim,
	selection: &Selection,
) -> Result<Vec<&'c ResolvedSource>, MutateError> {
	let names: Vec<(&str, Option<bool>)> = match selection {
		Selection::All => return Ok(claim.sources.iter().collect()),
		Selection::Sources(names) => {
			names.iter().map(|name| (name.as_str(), None)).collect()
		}
		Selection::Declared(sources) => sources
			.iter()
			.map(|source| match source {
				SourceSpec::Path(path) => (path.as_str(), Some(false)),
				SourceSpec::Pattern(pattern) => (pattern.as_str(), Some(true)),
			})
			.collect(),
	};
	if names.is_empty() {
		return Err(MutateError::NothingSelected {
			document: claim.document.clone(),
		});
	}
	let mut chosen = BTreeSet::new();
	for (name, pattern) in names {
		let mut found = false;
		for source in &claim.sources {
			let matches = pattern.map_or_else(
				|| names_source(claim, source, name),
				|pattern| {
					declared(claim, source.index()) == name
						&& matches!(source, ResolvedSource::Pattern(_)) == pattern
				},
			);
			if matches {
				chosen.insert(source.index());
				found = true;
			}
		}
		if !found {
			return Err(MutateError::UnknownSource {
				document: claim.document.clone(),
				selector: name.to_owned(),
				declared: claim
					.declaration
					.sources
					.iter()
					.map(|source| source.declared().to_owned())
					.collect(),
			});
		}
	}
	Ok(
		claim
			.sources
			.iter()
			.filter(|source| chosen.contains(&source.index()))
			.collect(),
	)
}

/// Whether `name` names `source` of `claim`.
fn names_source(claim: &Claim, source: &ResolvedSource, name: &str) -> bool {
	let declared = claim
		.declaration
		.sources
		.get(source.index())
		.map(SourceDecl::declared);
	if declared == Some(name) {
		return true;
	}
	if !name.starts_with('/') {
		return false;
	}
	match source {
		ResolvedSource::Path(source) => {
			resolve_rooted(name).is_ok_and(|path| path == source.path)
		}
		ResolvedSource::Pattern(source) => source.glob == name,
	}
}

/// The source of `claim` as declared at `index`.
pub(super) fn declared(claim: &Claim, index: usize) -> String {
	claim
		.declaration
		.sources
		.get(index)
		.map(|source| source.declared().to_owned())
		.unwrap_or_default()
}

/// Observes the sources of `claim` that `selection` names.
///
/// `claim` must not be a sealed record, whose patterns are not expanded.
///
/// # Errors
///
/// Fails as [`select`] does; with [`MutateError::MissingSource`] for a
/// selected path source that the snapshot lacks; with
/// [`MutateError::Sealed`] for a pattern that was not expanded; with
/// [`MutateError::NotCanonical`] for a file without a canonical oid; and when
/// the backend fails.
pub(super) fn observe<S: Snapshot>(
	repo: &Repo<'_, S>,
	claim: &Claim,
	selection: &Selection,
) -> Result<Observed, MutateError> {
	let selected = select(claim, selection)?;
	let mut files = BTreeSet::new();
	for source in &selected {
		match source {
			ResolvedSource::Path(source) => {
				if !source.exists {
					return Err(MutateError::MissingSource {
						document: claim.document.clone(),
						declared: declared(claim, source.index),
						path: source.path.clone(),
					});
				}
				files.insert(source.path.clone());
			}
			ResolvedSource::Pattern(source) => {
				let matches =
					source.matches.as_ref().ok_or_else(|| MutateError::Sealed {
						document: claim.document.clone(),
					})?;
				files.extend(matches.iter().cloned());
			}
		}
	}
	let files: Vec<RepoPath> = files.into_iter().collect();
	let oids = repo.oids(&files)?;
	let current: BTreeMap<RepoPath, ObjectId> =
		files.into_iter().zip(oids).collect();
	let sources = selected
		.into_iter()
		.filter_map(|source| observe_one(claim, source, &current))
		.collect();
	Ok(Observed { sources })
}

/// One selected source, whose files all have an oid in `current`.
fn observe_one(
	claim: &Claim,
	source: &ResolvedSource,
	current: &BTreeMap<RepoPath, ObjectId>,
) -> Option<ObservedSource> {
	let state = match source {
		ResolvedSource::Path(source) => SourceState::Path {
			path: source.path.clone(),
			attestation: source.attestation.clone(),
			current: current.get(&source.path)?.clone(),
		},
		ResolvedSource::Pattern(source) => SourceState::Pattern {
			matches: source
				.matches
				.iter()
				.flatten()
				.filter_map(|path| {
					current.get(path).map(|oid| (path.clone(), oid.clone()))
				})
				.collect(),
			members: source.members.clone(),
		},
	};
	Some(ObservedSource {
		index: source.index(),
		declared: declared(claim, source.index()),
		state,
	})
}
