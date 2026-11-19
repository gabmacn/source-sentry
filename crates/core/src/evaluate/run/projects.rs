//! Freshness across project boundaries, without applying foreign policy.

use std::collections::{BTreeMap, BTreeSet};

use jiff::Timestamp;

use super::{
	Coverage, Elsewhere, Evaluation, SubjectKind, check_within, judge_all,
	propagate, unloadable,
};
use crate::claims::{Diagnostic, Inventory, discover, graph};
use crate::config::{ConfigSet, LoadError, project_of};
use crate::model::{DocumentId, RepoPath};
use crate::ports::{BackendError, History, Snapshot};

/// Another project that the checked project cites, with the documents of it
/// that are cited.
struct Cited {
	configs: ConfigSet,
	found: Inventory,
	documents: BTreeSet<DocumentId>,
}

/// Evaluates one project, including the freshness of source documents in other
/// projects. Foreign contexts are never applied, and foreign subjects are not
/// reported.
///
/// Of another project, only the cited documents are evaluated, with the
/// documents of that project that they cite in turn: their own sources are
/// hashed and their own clocks read, and nothing else of that project. Every
/// diagnostic of that evaluation is reported, among them the project's
/// configuration errors and the errors met while hashing the cited
/// documents' sources, as are the configuration errors of a project that
/// cannot be loaded.
///
/// The other projects are discovered before any evaluation, so that a
/// drifted source that is a renamed document of another project is followed
/// to its earlier paths, as a source of the same project is.
///
/// # Errors
///
/// Fails when the snapshot or history cannot be read.
pub fn check<S: Snapshot, H: History>(
	snapshot: &S,
	history: &H,
	configs: &ConfigSet,
	inventory: &Inventory,
	at: Timestamp,
	context: Option<&str>,
) -> Result<Evaluation, BackendError> {
	let Discovered {
		mut combined,
		loaded,
		unloaded,
	} = discover_cited(snapshot, configs, inventory, at)?;
	let elsewhere = Elsewhere {
		projects: std::iter::once((configs, inventory))
			.chain(
				loaded
					.values()
					.flatten()
					.map(|cited| (&cited.configs, &cited.found)),
			)
			.collect(),
	};
	let mut result = check_within(
		snapshot,
		history,
		configs,
		inventory,
		at,
		Coverage::Project(context),
		&elsewhere,
	)?;
	result.diagnostics.extend(unloaded);
	let mut foreign = Vec::new();
	for cited in loaded.values().flatten() {
		if cited.documents.is_empty() {
			continue;
		}
		let evaluated = check_within(
			snapshot,
			history,
			&cited.configs,
			&narrowed(&cited.found, &cited.documents),
			at,
			Coverage::Cited(&cited.documents),
			&elsewhere,
		)?;
		foreign.extend(evaluated.subjects.into_iter().filter(|subject| {
			matches!(&subject.kind, SubjectKind::Document(document)
				if cited.documents.contains(&document.document))
		}));
		result.diagnostics.extend(evaluated.diagnostics);
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

/// What [`discover_cited`] finds: the other projects that a project cites,
/// before any of them is evaluated.
struct Discovered {
	/// The checked project's claims, with those of every cited document.
	combined: Inventory,
	/// Each other project whose documents are cited, by root, or `None`
	/// when it cannot be loaded.
	loaded: BTreeMap<RepoPath, Option<Cited>>,
	/// The configuration errors of the projects that cannot be loaded.
	unloaded: Vec<Diagnostic>,
}

/// The other projects whose documents the claims of `inventory` cite, and
/// those that the cited documents cite in turn, each loaded and discovered
/// once, with the documents of it that are cited. Nothing is evaluated.
///
/// # Errors
///
/// Fails when the snapshot cannot be read.
fn discover_cited<S: Snapshot>(
	snapshot: &S,
	configs: &ConfigSet,
	inventory: &Inventory,
	at: Timestamp,
) -> Result<Discovered, BackendError> {
	let mut unloaded = Vec::new();
	let mut combined = inventory.clone();
	let mut loaded: BTreeMap<RepoPath, Option<Cited>> = BTreeMap::new();
	let mut visited = BTreeSet::new();
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
						unloaded
							.extend(unloadable(snapshot.revision(), at, &errors).diagnostics);
						loaded.insert(project, None);
						continue;
					}
				};
				let found = discover(snapshot, &other)?;
				loaded.insert(
					project.clone(),
					Some(Cited {
						configs: other,
						found,
						documents: BTreeSet::new(),
					}),
				);
			}
			let Some(Some(cited)) = loaded.get_mut(&project) else {
				continue;
			};
			let document = DocumentId::Local(path);
			let before = combined.claims.len();
			combined
				.claims
				.extend(cited.found.claims_of(&document).cloned());
			// A declaration that cannot be read leaves no claim, only its
			// diagnostic, which must still reach the citing project.
			let one = BTreeSet::from([document]);
			if combined.claims.len() > before
				|| cited
					.found
					.diagnostics
					.iter()
					.any(|diagnostic| concerns(diagnostic, &one))
			{
				cited.documents.extend(one);
			}
		}
	}
	Ok(Discovered {
		combined,
		loaded,
		unloaded,
	})
}

/// `found` narrowed to the claims of `cited` documents: no other claim and
/// no unclaimed file, and of discovery's diagnostics only those that
/// [concern](concerns) a cited document. Lockfiles, tombstones, dependencies and series stay whole,
/// because the cited documents' declaration sites, upstreams and series
/// membership are read from them.
pub(super) fn narrowed(
	found: &Inventory,
	cited: &BTreeSet<DocumentId>,
) -> Inventory {
	Inventory {
		project: found.project.clone(),
		projects: found.projects.clone(),
		revision: found.revision.clone(),
		lockfiles: found.lockfiles.clone(),
		tombstones: found.tombstones.clone(),
		dependencies: found.dependencies.clone(),
		series: found.series.clone(),
		claims: found
			.claims
			.iter()
			.filter(|claim| cited.contains(&claim.document))
			.cloned()
			.collect(),
		unclaimed: Vec::new(),
		diagnostics: found
			.diagnostics
			.iter()
			.filter(|diagnostic| concerns(diagnostic, cited))
			.cloned()
			.collect(),
	}
}

/// Whether `diagnostic` concerns one of the `cited` documents: it is about
/// one, or it is about a document and names a cited one among its other
/// places, as a duplicate version names every member of its group but is
/// about the first.
fn concerns(diagnostic: &Diagnostic, cited: &BTreeSet<DocumentId>) -> bool {
	let Some(document) = &diagnostic.document else {
		return false;
	};
	cited.contains(document)
		|| diagnostic
			.related
			.iter()
			.any(|location| cited.contains(&DocumentId::Local(location.file.clone())))
}
