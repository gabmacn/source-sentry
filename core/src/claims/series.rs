//! Series membership: which files belong to which series, in version order.

use std::collections::{BTreeMap, BTreeSet};
use std::iter;

use super::{
	Claim, Diagnostic, DiagnosticKind, Directories, Location, MemberState,
	Presence, SeriesMember, SeriesMembers, Tombstone, Version, in_tool_directory,
	is_own_file,
};
use crate::config::{ConfigSet, Scope, Series};
use crate::model::{DocumentId, RepoPath};

/// A series' identity: its scope plus its name.
type Key = (Scope, String);

/// Every series in effect anywhere, with its members ordered by version.
///
/// The series are those of the repository config and of every directory
/// config, each with its imports. A path's series are those in effect for
/// its directory whose pattern matches it ([`ConfigSet::settings`] and
/// `series_of`). The candidates are:
///
/// - every tracked file outside `.source-sentry/` that is not a lockfile or
///   a directory config, eligible or not, so that an ignore can never hide a
///   newer entry and revive an older one;
/// - every local document that a lockfile lists but the snapshot lacks, as
///   [`MemberState::Missing`], so that a missing head stays visible;
/// - every local path that only a tombstone names, as
///   [`MemberState::Retired`].
///
/// A file that matches two series, or whose version does not parse, is
/// reported and left out. Members with equal versions are reported and kept.
pub(super) fn membership(
	tracked: &BTreeSet<RepoPath>,
	claims: &[Claim],
	tombstones: &[Tombstone],
	configs: &ConfigSet,
	directories: &mut Directories<'_>,
	diagnostics: &mut Vec<Diagnostic>,
) -> Vec<SeriesMembers> {
	let mut series = every_series(configs, directories);
	let claimed: BTreeSet<&RepoPath> = claims
		.iter()
		.filter_map(|claim| local(&claim.document))
		.collect();
	for (path, state) in candidates(tracked, claims, tombstones) {
		let matches: Vec<(Key, Series, String)> = directories
			.of(&path)
			.series_of(&path)
			.into_iter()
			.map(|(series, version)| {
				(key(series), series.clone(), version.to_owned())
			})
			.collect();
		match matches.as_slice() {
			[] => {}
			[(key, definition, text)] => {
				match Version::parse(definition.version_kind, text) {
					Ok(version) => {
						series
							.entry(key.clone())
							.or_insert_with(|| SeriesMembers {
								series: definition.clone(),
								members: Vec::new(),
							})
							.members
							.push(SeriesMember {
								claimed: claimed.contains(&path),
								path,
								version,
								state,
							});
					}
					Err(error) => diagnostics.push(
						Diagnostic::new(
							DiagnosticKind::InvalidVersion,
							Location::file(path.clone()),
							format!(
								"the series `{}` reads a version from this file's path, but \
								 {error}. Rename the file, or correct the series' pattern",
								definition.name
							),
						)
						.with_related(definition_location(definition).into_iter().collect())
						.about(DocumentId::Local(path)),
					),
				}
			}
			[_, _, ..] => diagnostics.push(overlap(&path, &matches)),
		}
	}
	let mut ordered: Vec<SeriesMembers> = series.into_values().collect();
	for members in &mut ordered {
		members.members.sort_by(|left, right| {
			(&left.version, &left.path).cmp(&(&right.version, &right.path))
		});
		report_duplicates(members, diagnostics);
	}
	ordered
}

/// A series' identity.
fn key(series: &Series) -> Key {
	(series.scope.clone(), series.name.clone())
}

/// The path of a local document.
fn local(document: &DocumentId) -> Option<&RepoPath> {
	match document {
		DocumentId::Local(path) => Some(path),
		DocumentId::Remote(_) => None,
	}
}

/// Every series in effect for the repository root or for a directory with a
/// config, which together are every series any config defines or imports.
fn every_series(
	configs: &ConfigSet,
	directories: &mut Directories<'_>,
) -> BTreeMap<Key, SeriesMembers> {
	let mut series = BTreeMap::new();
	let roots: Vec<RepoPath> = iter::once(RepoPath::ROOT)
		.chain(configs.config_directories().cloned())
		.collect();
	for directory in roots {
		for definition in &directories.directory(&directory).series {
			series
				.entry(key(definition))
				.or_insert_with(|| SeriesMembers {
					series: definition.clone(),
					members: Vec::new(),
				});
		}
	}
	series
}

/// The paths that may be series members, each with its state.
fn candidates(
	tracked: &BTreeSet<RepoPath>,
	claims: &[Claim],
	tombstones: &[Tombstone],
) -> BTreeMap<RepoPath, MemberState> {
	let mut candidates: BTreeMap<RepoPath, MemberState> = tracked
		.iter()
		.filter(|path| !in_tool_directory(path) && !is_own_file(path))
		.map(|path| (path.clone(), MemberState::Present))
		.collect();
	let missing = claims
		.iter()
		.filter(|claim| claim.presence == Presence::Missing)
		.filter_map(|claim| local(&claim.document));
	for path in missing {
		candidates
			.entry(path.clone())
			.or_insert(MemberState::Missing);
	}
	let retired = tombstones
		.iter()
		.filter(|tombstone| tombstone.presence == Presence::Missing)
		.filter_map(|tombstone| local(&tombstone.document));
	for path in retired {
		candidates
			.entry(path.clone())
			.or_insert(MemberState::Retired);
	}
	candidates
}

/// Where a series is defined, when a file defines it.
fn definition_location(series: &Series) -> Option<Location> {
	let file = series.origin.file.clone()?;
	let position = series.origin.position;
	Some(Location::field(file, format!("series.{}", series.name)).at(
		position.map(|position| position.line),
		position.map(|position| position.column),
	))
}

/// The diagnostic for a path that matches more than one series.
fn overlap(path: &RepoPath, matches: &[(Key, Series, String)]) -> Diagnostic {
	let names: Vec<String> = matches
		.iter()
		.map(|(_, series, _)| format!("`{}` ({})", series.name, series.scope))
		.collect();
	Diagnostic::new(
		DiagnosticKind::SeriesOverlap,
		Location::file(path.clone()),
		format!(
			"`{path}` matches the series {}, but a file may belong to one series \
			 only. Narrow the patterns so that only one matches, or rename the file",
			names.join(" and ")
		),
	)
	.with_related(
		matches
			.iter()
			.filter_map(|(_, series, _)| definition_location(series))
			.collect(),
	)
	.about(DocumentId::Local(path.clone()))
}

/// Reports every group of members with the same version.
/// `members` must be sorted by version.
fn report_duplicates(
	members: &SeriesMembers,
	diagnostics: &mut Vec<Diagnostic>,
) {
	for group in members
		.members
		.chunk_by(|left, right| left.version == right.version)
	{
		let [first, rest @ ..] = group else {
			continue;
		};
		if rest.is_empty() {
			continue;
		}
		let names: Vec<String> = group
			.iter()
			.map(|member| format!("`{}`", member.path))
			.collect();
		let mut related: Vec<Location> = rest
			.iter()
			.map(|member| Location::file(member.path.clone()))
			.collect();
		related.extend(definition_location(&members.series));
		diagnostics.push(
			Diagnostic::new(
				DiagnosticKind::DuplicateVersion,
				Location::file(first.path.clone()),
				format!(
					"{} all have version {} in the series `{}`, but versions in a \
					 series must differ. Rename all but one",
					names.join(", "),
					first.version,
					members.series.name
				),
			)
			.with_related(related)
			.about(DocumentId::Local(first.path.clone())),
		);
	}
}
