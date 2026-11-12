//! Discovery: claims, roots, unclaimed files and series membership.
//!
//! [`discover`] reads a [`Snapshot`] with its loaded [`ConfigSet`] and builds
//! the [`Inventory`] that evaluation, the mutation commands and the report
//! work from. It neither hashes nor reads history; it reads lockfiles and
//! declaration-capable documents through [`Snapshot::read`].
//!
//! # Claims
//!
//! A document is claimed on-file, by its own frontmatter, or by a lockfile:
//!
//! - **Local lockfiles** are every tracked `source-sentry.lock` outside
//!   `.source-sentry/`. Only the nearest ancestor lockfile may list a
//!   document.
//! - **Remote owner lockfiles** are `.source-sentry/remotes/<owner>.lock`,
//!   direct children only.
//! - **On-file declarations** are read from every tracked file whose adapter
//!   is `md-utf8`, eligible or not, because ignores never affect the
//!   validation of existing claims. Nothing under
//!   `.source-sentry/` is read for one.
//!
//! A document claimed twice, or a remote identity in two owner lockfiles, is
//! an integrity error. A listed local document that the
//! snapshot lacks is a configuration error, or an integrity error for a
//! sealed record.
//!
//! # Sources and dependencies
//!
//! Each claim's sources are resolved against the snapshot: path sources with
//! [`resolve_source`](crate::model::resolve_source) or, for a remote
//! document, [`resolve_rooted`](crate::model::resolve_rooted); pattern
//! sources as globs over the snapshot's tracked paths,
//! keeping both the current matches and the attested members. A source that
//! is itself a lockfile-tracked document becomes a [`Dependency`]; one that
//! is an on-file document, a cycle, and a self-dependency are configuration
//! errors. A sealed record's sources are frozen, so its patterns
//! are not expanded and it adds no dependency.
//!
//! # Unclaimed files and series
//!
//! A root's scan covers its subtree minus the subtrees of nested roots, and
//! finds eligible files that nothing claims. Series
//! membership comes from the series in effect for each file's directory,
//! with versions parsed by kind and ordered by value.
//!
//! # Diagnostics
//!
//! Problems never stop discovery. Each becomes a [`Diagnostic`] with a
//! stable [`DiagnosticKind`] and its [`Severity`]: a configuration or
//! integrity error, both of which exit 3 and map to the policy module's
//! [`ErrorKind`](crate::policy::ErrorKind), or a non-fatal warning. Only a
//! failure of the snapshot itself stops discovery, as a [`BackendError`]
//! (exit 4).
//!
//! Paths that differ only in case are diagnosed here. Submodules
//! and the other canonicalization problems surface only through
//! [`Snapshot::canonical_oids`], which discovery never calls, so the step that
//! hashes reports them.

mod case;
mod diagnostic;
pub(crate) mod graph;
mod inventory;
mod pattern;
mod read;
mod series;
mod sources;
mod stranded;
mod unclaimed;
mod validate;
mod version;

use std::collections::{BTreeMap, BTreeSet};

pub use diagnostic::{Diagnostic, DiagnosticKind, Location, Severity};
pub use inventory::{
	Attestation, Claim, Dependency, Inventory, LockfileInfo, LockfileKind,
	MemberState, Mode, Presence, ResolvedPath, ResolvedPattern, ResolvedSource,
	SeriesMember, SeriesMembers, Tombstone, Unclaimed,
};
pub use pattern::{PatternProblem, SourceGlob};
pub use read::{FRONTMATTER_FIELD, entry_field};
pub use version::{Version, VersionError};

use crate::config::{ConfigSet, DirectorySettings};
use crate::model::{DocumentId, RepoPath};
use crate::ports::{BackendError, Snapshot};
use crate::{CONFIG_NAME, DIR_NAME, LOCKFILE_NAME};

/// Builds the inventory of `snapshot`, whose configs `configs` holds.
///
/// `configs` must have been loaded from the same snapshot; its errors are
/// reported by [`ConfigSet::load`], so a snapshot with invalid configs never
/// reaches discovery. Every problem with claims, sources, series or paths is
/// collected into [`Inventory::diagnostics`], and discovery always
/// continues past it.
///
/// # Errors
///
/// Fails only when the snapshot cannot list its paths or read a lockfile or
/// a declaration-capable document. That is an operational error: the
/// evaluation could not complete (exit 4).
pub fn discover<S: Snapshot>(
	snapshot: &S,
	configs: &ConfigSet,
) -> Result<Inventory, BackendError> {
	let tracked: BTreeSet<RepoPath> = snapshot.paths()?.into_iter().collect();
	let owned: BTreeSet<RepoPath> = tracked
		.iter()
		.filter(|path| configs.owns(path))
		.cloned()
		.collect();
	let paths: Vec<RepoPath> = owned.iter().cloned().collect();
	let mut directories = Directories::new(configs);
	let mut diagnostics = Vec::new();

	let mut found = read::Found::default();
	read::lockfiles(
		snapshot,
		&paths,
		&tracked,
		configs.project(),
		&mut found,
		&mut diagnostics,
	)?;
	read::on_file(
		snapshot,
		&paths,
		&mut directories,
		&mut found,
		&mut diagnostics,
	)?;
	stranded::read(snapshot, configs, &tracked, &mut found)?;
	let read::Found {
		lockfiles,
		mut claims,
		mut tombstones,
	} = found;
	claims.sort_by(|left, right| {
		(&left.document, &left.declared_in, &left.field).cmp(&(
			&right.document,
			&right.declared_in,
			&right.field,
		))
	});
	tombstones.sort_by(|left, right| {
		(&left.document, &left.lockfile).cmp(&(&right.document, &right.lockfile))
	});

	validate::validate(
		&lockfiles,
		&claims,
		&tombstones,
		configs,
		&mut diagnostics,
	);
	sources::resolve(&mut claims, &tracked, configs.projects(), &mut diagnostics);
	let dependencies = graph::dependencies(&claims, &mut diagnostics);
	let unclaimed = unclaimed::scan(&owned, &claims, configs, &mut directories);
	let series = series::membership(
		&owned,
		&claims,
		&tombstones,
		configs,
		&mut directories,
		&mut diagnostics,
	);
	case::collisions(&owned, &mut diagnostics);

	diagnostics.sort_by(|left, right| left.sort_key().cmp(&right.sort_key()));
	diagnostics.dedup();
	let mut inventory = Inventory {
		project: configs.project().clone(),
		projects: configs.projects().clone(),
		revision: snapshot.revision().clone(),
		lockfiles,
		claims,
		tombstones,
		dependencies,
		unclaimed,
		series,
		diagnostics,
	};
	for index in 0..inventory.unclaimed.len() {
		let predecessor = inventory
			.predecessor(&inventory.unclaimed[index].path)
			.and_then(|claim| match &claim.document {
				DocumentId::Local(path) => Some(path.clone()),
				DocumentId::Remote(_) => None,
			});
		inventory.unclaimed[index].inherit_from = predecessor;
	}
	Ok(inventory)
}

/// Each directory's effective settings, computed once per discovery.
struct Directories<'c> {
	configs: &'c ConfigSet,
	settings: BTreeMap<RepoPath, DirectorySettings>,
}

impl<'c> Directories<'c> {
	fn new(configs: &'c ConfigSet) -> Self {
		Self {
			configs,
			settings: BTreeMap::new(),
		}
	}

	/// The settings of `directory`.
	fn directory(&mut self, directory: &RepoPath) -> &DirectorySettings {
		let configs = self.configs;
		self
			.settings
			.entry(directory.clone())
			.or_insert_with_key(|directory| configs.settings(directory))
	}

	/// The settings of the directory that holds `file`.
	fn of(&mut self, file: &RepoPath) -> &DirectorySettings {
		self.directory(&file.parent().unwrap_or(RepoPath::ROOT))
	}
}

/// What a file that git does not track would be, once added.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outside {
	/// One of Source Sentry's own files: anything under `.source-sentry/`, a
	/// lockfile or a directory config.
	Own,
	/// A file that a root's scan would take as a document.
	Document,
}

/// The files among `paths`, which git does not track, that would count once
/// added, in the order given. `check` reads only the index, so these are
/// the files it silently leaves out.
#[must_use]
pub fn outside_the_index(
	paths: &[RepoPath],
	configs: &ConfigSet,
) -> Vec<(RepoPath, Outside)> {
	let mut directories = Directories::new(configs);
	paths
		.iter()
		.filter(|path| configs.owns(path))
		.filter_map(|path| {
			let outside = if in_tool_directory(path) || is_own_file(path) {
				Outside::Own
			} else if configs.root_of(path).is_some()
				&& directories.of(path).eligibility(path).is_eligible()
			{
				Outside::Document
			} else {
				return None;
			};
			Some((path.clone(), outside))
		})
		.collect()
}

/// Whether `path` lies under `.source-sentry/`, which holds Source
/// Sentry's own files rather than documents.
fn in_tool_directory(path: &RepoPath) -> bool {
	path.components().any(|part| part == DIR_NAME)
}

/// Whether `path` is a lockfile or a directory config, which are never
/// documents.
fn is_own_file(path: &RepoPath) -> bool {
	matches!(path.file_name(), Some(LOCKFILE_NAME | CONFIG_NAME))
}

#[cfg(test)]
mod tests;
