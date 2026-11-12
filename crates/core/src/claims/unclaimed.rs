//! Unclaimed files: eligible files inside a root that nothing claims.

use std::collections::BTreeSet;

use super::{Claim, Directories, Unclaimed, in_tool_directory, is_own_file};
use crate::config::ConfigSet;
use crate::model::{DocumentId, RepoPath};

/// Every tracked file that is unclaimed, sorted by path.
///
/// A file is unclaimed when all of the following hold:
///
/// - a root's scan covers it: the nearest root above it is
///   [`ConfigSet::root_of`], so a scan stops where a nested root begins and
///   continues through a directory with `root: false`;
/// - it is eligible as a document under its directory's settings
///   (`include` minus `ignore`);
/// - no declaration anywhere claims it, valid or not, so a path that only a
///   tombstone names counts once it exists again;
/// - it is not Source Sentry's own: not under `.source-sentry/`, and not a
///   lockfile or a directory config.
///
/// Remote documents have no path, so they are never unclaimed.
pub(super) fn scan(
	tracked: &BTreeSet<RepoPath>,
	claims: &[Claim],
	configs: &ConfigSet,
	directories: &mut Directories<'_>,
) -> Vec<Unclaimed> {
	let claimed: BTreeSet<&RepoPath> = claims
		.iter()
		.filter_map(|claim| match &claim.document {
			DocumentId::Local(path) => Some(path),
			DocumentId::Remote(_) => None,
		})
		.collect();
	let mut unclaimed = Vec::new();
	for path in tracked {
		if in_tool_directory(path) || is_own_file(path) || claimed.contains(path) {
			continue;
		}
		let Some(root) = configs.root_of(path) else {
			continue;
		};
		if directories.of(path).eligibility(path).is_eligible() {
			unclaimed.push(Unclaimed {
				inherit_from: None,
				path: path.clone(),
				root,
			});
		}
	}
	unclaimed
}
