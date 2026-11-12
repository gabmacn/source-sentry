//! Tracked paths that differ only in case.

use std::collections::{BTreeMap, BTreeSet};

use super::{Diagnostic, DiagnosticKind, Location};
use crate::model::RepoPath;

/// Reports every set of tracked files or directories, in one directory,
/// whose names differ only in case. On a case-insensitive file system they
/// would be the same path, so a checkout there would lose one of them.
///
/// Directories are compared as well as files, and only siblings are
/// compared: `Docs/a.md` and `docs/a.md` are reported once, as the
/// directories `Docs` and `docs`. Case is folded with Unicode lowercasing.
pub(super) fn collisions(
	tracked: &BTreeSet<RepoPath>,
	diagnostics: &mut Vec<Diagnostic>,
) {
	let mut entries = BTreeSet::new();
	for path in tracked {
		let mut current = Some(path.clone());
		while let Some(entry) = current.filter(|entry| !entry.is_root()) {
			current = entry.parent();
			if !entries.insert(entry) {
				// Its ancestors are in already.
				break;
			}
		}
	}
	let mut siblings: BTreeMap<(RepoPath, String), Vec<RepoPath>> =
		BTreeMap::new();
	for entry in entries {
		let parent = entry.parent().unwrap_or(RepoPath::ROOT);
		let folded = entry.file_name().unwrap_or_default().to_lowercase();
		siblings.entry((parent, folded)).or_default().push(entry);
	}
	for group in siblings.into_values() {
		let [first, rest @ ..] = group.as_slice() else {
			continue;
		};
		if rest.is_empty() {
			continue;
		}
		let names: Vec<String> =
			group.iter().map(|entry| format!("`{entry}`")).collect();
		diagnostics.push(
			Diagnostic::new(
				DiagnosticKind::CaseCollision,
				Location::file(first.clone()),
				format!(
					"{} differ only in case, so a case-insensitive file system, such \
					 as the default on Windows and macOS, would see one path. Rename \
					 all but one",
					names.join(" and ")
				),
			)
			.with_related(rest.iter().cloned().map(Location::file).collect()),
		);
	}
}
