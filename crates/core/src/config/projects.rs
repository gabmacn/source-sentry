//! Project markers and nearest-project ownership within one repository.

use std::collections::BTreeSet;

use crate::model::RepoPath;
use crate::{CONFIG_NAME, DIR_NAME};

/// The project roots among tracked paths, including the repository root.
#[must_use]
pub fn project_roots(paths: &[RepoPath]) -> BTreeSet<RepoPath> {
	let mut roots = BTreeSet::from([RepoPath::ROOT]);
	for path in paths {
		if path.file_name() == Some(CONFIG_NAME)
			&& let Some(parent) = path.parent()
			&& parent.file_name() == Some(DIR_NAME)
			&& let Some(root) = parent.parent()
		{
			roots.insert(root);
		}
	}
	roots
}

/// The nearest project containing `path`; roots always include the repository.
#[must_use]
pub fn project_of(roots: &BTreeSet<RepoPath>, path: &RepoPath) -> RepoPath {
	roots
		.iter()
		.filter(|root| path.starts_with(root))
		.max_by_key(|root| root.components().count())
		.cloned()
		.unwrap_or(RepoPath::ROOT)
}
