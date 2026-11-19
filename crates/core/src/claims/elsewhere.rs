//! Remote identities that other projects' owner lockfiles track.

use std::collections::{BTreeMap, BTreeSet};

use super::read::{self, lockfile_kind};
use super::{Location, LockfileKind};
use crate::DIR_NAME;
use crate::config::{ConfigSet, project_of};
use crate::model::{DocumentId, RepoPath};
use crate::ports::{BackendError, Snapshot};

/// Reads the owner lockfiles of every project but the one `configs`
/// governs, and returns each remote identity they track with the entries
/// that track it.
///
/// Only the identities matter here, so nothing else of those projects is
/// read, and none of their configs. An owner lockfile that does not parse
/// is passed over without a diagnostic: it is that project's fault, which
/// its own check reports, and a fault of another project never reaches this
/// one's report.
///
/// # Errors
///
/// Fails when the snapshot cannot read an owner lockfile.
pub(super) fn read<S: Snapshot>(
	snapshot: &S,
	configs: &ConfigSet,
	tracked: &BTreeSet<RepoPath>,
) -> Result<BTreeMap<DocumentId, BTreeSet<Location>>, BackendError> {
	let mut owners: BTreeMap<RepoPath, Vec<RepoPath>> = BTreeMap::new();
	for path in tracked
		.iter()
		.filter(|path| path.components().any(|part| part == DIR_NAME))
	{
		let project = project_of(configs.projects(), path);
		if &project == configs.project() {
			continue;
		}
		let relative = path.strip_prefix(&project).unwrap_or(path.as_str());
		let remote = RepoPath::new(relative)
			.ok()
			.and_then(|relative| lockfile_kind(&relative))
			.is_some_and(|kind| matches!(kind, LockfileKind::Remote { .. }));
		if remote {
			owners.entry(project).or_default().push(path.clone());
		}
	}
	let mut elsewhere: BTreeMap<DocumentId, BTreeSet<Location>> = BTreeMap::new();
	for (project, paths) in owners {
		let mut found = read::Found::default();
		read::lockfiles(
			snapshot,
			&paths,
			tracked,
			&project,
			&mut found,
			&mut Vec::new(),
		)?;
		for claim in found.claims {
			let location = claim.location();
			elsewhere
				.entry(claim.document)
				.or_default()
				.insert(location);
		}
	}
	Ok(elsewhere)
}
