//! Declarations left in enclosing lockfiles when a new project appears.

use std::collections::BTreeSet;

use super::{Diagnostic, read};
use crate::LOCKFILE_NAME;
use crate::config::{ConfigSet, project_of};
use crate::model::{DocumentId, RepoPath};
use crate::ports::{BackendError, Snapshot};

/// Reads only enclosing lockfiles that can contain this project's documents.
/// Unrelated outer entries stay outside this project's inventory. An
/// enclosing lockfile that does not parse could hide declarations of this
/// project's documents, so its diagnostic joins `diagnostics`.
pub(super) fn read<S: Snapshot>(
	snapshot: &S,
	configs: &ConfigSet,
	tracked: &BTreeSet<RepoPath>,
	found: &mut read::Found,
	diagnostics: &mut Vec<Diagnostic>,
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
		let mut problems = Vec::new();
		read::lockfiles(
			snapshot,
			std::slice::from_ref(path),
			tracked,
			&project,
			&mut outer,
			&mut problems,
		)?;
		if outer.lockfiles.iter().any(|lockfile| !lockfile.parsed) {
			diagnostics.extend(problems);
			continue;
		}
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
