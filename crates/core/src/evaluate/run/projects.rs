//! Freshness across project boundaries, without applying foreign policy.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};

use super::{
	Evaluation, SubjectKind, check_local, judge_all, propagate, unloadable,
};
use crate::claims::{Inventory, discover, graph};
use crate::config::{ConfigSet, LoadError, project_of};
use crate::model::{DocumentId, RepoPath};
use crate::ports::{BackendError, History, Snapshot};

/// Evaluates one project, including the freshness of source documents in other
/// projects. Foreign contexts are never applied, and foreign subjects are not
/// reported. Only referenced foreign declarations contribute diagnostics.
///
/// # Errors
///
/// Fails when the snapshot or history cannot be read.
pub fn check<S: Snapshot, H: History>(
	snapshot: &S,
	history: &H,
	configs: &ConfigSet,
	inventory: &Inventory,
	at: DateTime<Utc>,
	context: Option<&str>,
) -> Result<Evaluation, BackendError> {
	let mut result =
		check_local(snapshot, history, configs, inventory, at, context, true)?;
	let mut combined = inventory.clone();
	let mut loaded = BTreeMap::new();
	let mut visited = BTreeSet::new();
	let mut foreign = Vec::new();
	loop {
		let needed: BTreeSet<RepoPath> = combined
			.claims
			.iter()
			.filter(|claim| !claim.is_sealed())
			.flat_map(|claim| claim.source_paths().into_iter().cloned())
			.filter(|path| !configs.owns(path) && !visited.contains(path))
			.collect();
		if needed.is_empty() {
			break;
		}
		for path in needed {
			visited.insert(path.clone());
			let project = project_of(configs.projects(), &path);
			if !loaded.contains_key(&project) {
				let other = match ConfigSet::load_project(snapshot, &project) {
					Ok(other) => other,
					Err(LoadError::Backend(error)) => return Err(error),
					Err(LoadError::Invalid(errors)) => {
						result
							.diagnostics
							.extend(unloadable(snapshot.revision(), at, &errors).diagnostics);
						loaded.insert(project, None);
						continue;
					}
				};
				let found = discover(snapshot, &other)?;
				let evaluated =
					check_local(snapshot, history, &other, &found, at, None, false)?;
				loaded.insert(project.clone(), Some((found, evaluated)));
			}
			let Some(Some((found, evaluated))) = loaded.get(&project) else {
				continue;
			};
			let document = DocumentId::Local(path);
			combined.claims.extend(found.claims_of(&document).cloned());
			foreign.extend(evaluated.subjects.iter().filter(|subject| matches!(&subject.kind, SubjectKind::Document(found) if found.document == document)).cloned());
			result.diagnostics.extend(
				evaluated
					.diagnostics
					.iter()
					.filter(|diagnostic| diagnostic.document.as_ref() == Some(&document))
					.cloned(),
			);
		}
	}
	combined
		.claims
		.sort_by(|left, right| left.document.cmp(&right.document));
	combined.dependencies =
		graph::dependencies(&combined.claims, &mut result.diagnostics);
	let count = result.subjects.len();
	result.subjects.extend(foreign);
	propagate(&mut result.subjects, &combined);
	result.subjects.truncate(count);
	result
		.diagnostics
		.sort_by(|left, right| left.sort_key().cmp(&right.sort_key()));
	result.diagnostics.dedup();
	result.summary = judge_all(
		&mut result.subjects,
		result.context.as_ref(),
		&result.diagnostics,
	);
	Ok(result)
}
