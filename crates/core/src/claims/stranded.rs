//! Declarations left in enclosing lockfiles when a new project appears.

use std::collections::BTreeSet;

use super::read;
use crate::LOCKFILE_NAME;
use crate::config::{ConfigSet, project_of};
use crate::model::{DocumentId, RepoPath};
use crate::ports::{BackendError, Snapshot};

/// Reads only enclosing lockfiles that can contain this project's documents.
/// Unrelated outer entries and errors stay outside this project's inventory.
pub(super) fn read<S: Snapshot>(
	snapshot: &S,
	configs: &ConfigSet,
	tracked: &BTreeSet<RepoPath>,
	found: &mut read::Found,
) -> Result<(), BackendError> {
	for path in tracked.iter().filter(|path| {
		path.file_name() == Some(LOCKFILE_NAME)
			&& !configs.owns(path)
			&& path
				.parent()
				.is_some_and(|directory| configs.project().starts_with(&directory))
	}) {
		let project = project_of(configs.projects(), path);
		let mut outer = read::Found::default();
		read::lockfiles(
			snapshot,
			std::slice::from_ref(path),
			tracked,
			&project,
			&mut outer,
			&mut Vec::new(),
		)?;
		outer.claims.retain(|claim| matches!(&claim.document, DocumentId::Local(path) if configs.owns(path)));
		outer.tombstones.retain(|tombstone| matches!(&tombstone.document, DocumentId::Local(path) if configs.owns(path)));
		if outer.claims.is_empty() && outer.tombstones.is_empty() {
			continue;
		}
		found.lockfiles.extend(outer.lockfiles);
		found.claims.extend(outer.claims);
		found.tombstones.extend(outer.tombstones);
	}
	Ok(())
}
