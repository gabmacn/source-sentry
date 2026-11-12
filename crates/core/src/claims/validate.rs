//! Validating claims and tombstones against each other, the lockfiles'
//! authority, the snapshot and the declared classes.

use std::collections::{BTreeMap, BTreeSet};

use super::inventory::nearest_lockfile;
use super::{
	Claim, Diagnostic, DiagnosticKind, Location, LockfileInfo, Mode, Presence,
	Tombstone,
};
use crate::config::ConfigSet;
use crate::evaluate::schedule::EffectiveSchedule;
use crate::model::DocumentId;

/// Reports double claims, identities that two owner lockfiles share, entries
/// outside their lockfile's authority, listed documents that the snapshot
/// lacks, undeclared classes, and invalid schedules, including those of
/// sealed records.
///
/// `claims` must be sorted by document.
pub(super) fn validate(
	lockfiles: &[LockfileInfo],
	claims: &[Claim],
	tombstones: &[Tombstone],
	configs: &ConfigSet,
	diagnostics: &mut Vec<Diagnostic>,
) {
	double_claims(claims, diagnostics);
	shared_remote_identities(claims, tombstones, diagnostics);
	authority(lockfiles, claims, tombstones, diagnostics);
	missing_documents(claims, diagnostics);
	undeclared_classes(claims, configs, diagnostics);
	schedules(claims, diagnostics);
}

/// Validates each written schedule before evaluation decides whether its
/// document needs a deadline. Inherited settings are validated in configs.
fn schedules(claims: &[Claim], diagnostics: &mut Vec<Diagnostic>) {
	for claim in claims {
		let Some(schedule) = &claim.declaration.schedule else {
			continue;
		};
		if let Err(error) = EffectiveSchedule::new(schedule) {
			let location =
				claim.location_of(format!("{}.schedule.cadence", claim.field));
			diagnostics.push(
				Diagnostic::schedule(&error, location).about(claim.document.clone()),
			);
		}
	}
}

/// How a claim's site reads in a message.
fn site(claim: &Claim) -> String {
	match claim.mode {
		Mode::OnFile => "its own frontmatter".to_owned(),
		Mode::LocalLockfile | Mode::RemoteLockfile { .. } => {
			format!("{} at `{}`", claim.declared_in, claim.field)
		}
	}
}

/// Reports every document claimed more than once.
fn double_claims(claims: &[Claim], diagnostics: &mut Vec<Diagnostic>) {
	for group in claims.chunk_by(|left, right| left.document == right.document) {
		let [first, rest @ ..] = group else {
			continue;
		};
		if rest.is_empty() {
			continue;
		}
		let sites: Vec<String> = group.iter().map(site).collect();
		diagnostics.push(
			Diagnostic::new(
				DiagnosticKind::DoubleClaim,
				first.location(),
				format!(
					"`{}` is claimed {} times: in {}. A document has exactly one \
					 declaration, so keep one and remove the others; `track --from` \
					 moves a declaration between modes and lockfiles",
					first.document,
					group.len(),
					sites.join(", and in ")
				),
			)
			.with_related(rest.iter().map(Claim::location).collect())
			.about(first.document.clone()),
		);
	}
}

/// Reports every remote identity that appears in more than one owner
/// lockfile with a tombstone in at least one of them. Two live
/// entries are a double claim instead.
fn shared_remote_identities(
	claims: &[Claim],
	tombstones: &[Tombstone],
	diagnostics: &mut Vec<Diagnostic>,
) {
	let mut lockfiles: BTreeMap<&DocumentId, BTreeSet<Location>> =
		BTreeMap::new();
	let remote =
		|document: &DocumentId| matches!(document, DocumentId::Remote(_));
	for tombstone in tombstones.iter().filter(|tomb| remote(&tomb.document)) {
		lockfiles
			.entry(&tombstone.document)
			.or_default()
			.insert(tombstone.location_of(tombstone.field.clone()));
	}
	for claim in claims {
		if let Some(locations) = lockfiles.get_mut(&claim.document) {
			locations.insert(claim.location());
		}
	}
	for (document, locations) in lockfiles {
		let files: BTreeSet<String> = locations
			.iter()
			.map(|location| location.file.to_string())
			.collect();
		if files.len() < 2 {
			continue;
		}
		let names: Vec<String> = files.into_iter().collect();
		let mut locations = locations.into_iter();
		let Some(first) = locations.next() else {
			continue;
		};
		diagnostics.push(
			Diagnostic::new(
				DiagnosticKind::RetiredInAnotherLockfile,
				first,
				format!(
					"`{document}` appears in {} owner lockfiles ({}), but a remote \
					 identity may appear in only one. Keep its entry and its \
					 tombstones in a single owner lockfile",
					names.len(),
					names.join(", ")
				),
			)
			.with_related(locations.collect())
			.about(document.clone()),
		);
	}
}

/// Reports every local entry and tombstone whose lockfile is not the
/// nearest ancestor lockfile of its document.
fn authority(
	lockfiles: &[LockfileInfo],
	claims: &[Claim],
	tombstones: &[Tombstone],
	diagnostics: &mut Vec<Diagnostic>,
) {
	let entries = claims
		.iter()
		.filter(|claim| claim.mode == Mode::LocalLockfile)
		.map(|claim| (&claim.document, claim.location(), "entry"));
	let retired = tombstones.iter().map(|tombstone| {
		let location = tombstone.location_of(tombstone.field.clone());
		(&tombstone.document, location, "tombstone")
	});
	for (document, location, what) in entries.chain(retired) {
		let DocumentId::Local(path) = document else {
			continue;
		};
		let Some(nearest) = nearest_lockfile(lockfiles, path) else {
			continue;
		};
		if nearest.path == location.file {
			continue;
		}
		diagnostics.push(
			Diagnostic::new(
				DiagnosticKind::OutsideAuthority,
				location.clone(),
				format!(
					"{} lists `{path}`, but {} is nearer to it, and only the nearest \
					 lockfile may list a document. Move this {what} there; `track \
					 --from` moves entries between lockfiles",
					location.file, nearest.path
				),
			)
			.with_related(vec![Location::file(nearest.path.clone())])
			.about(document.clone()),
		);
	}
}

/// Reports every document that a local lockfile lists but the snapshot
/// lacks. A tombstone replaces an entry rather than
/// sitting beside it, so a listed entry is reported even when a tombstone
/// for a previous document at the same path exists.
fn missing_documents(claims: &[Claim], diagnostics: &mut Vec<Diagnostic>) {
	for claim in claims {
		if claim.presence != Presence::Missing {
			continue;
		}
		let (kind, message) = if claim.is_sealed() {
			(
				DiagnosticKind::MissingRecord,
				format!(
					"the sealed record `{}` is not tracked in this snapshot. Restore \
					 it, or retire it with `retire`, whose tombstone keeps the seal",
					claim.document
				),
			)
		} else {
			(
				DiagnosticKind::MissingDocument,
				format!(
					"`{}` is listed here but is not tracked in this snapshot. Restore \
					 it, correct the entry, or retire it with `retire`",
					claim.document
				),
			)
		};
		diagnostics.push(
			Diagnostic::new(kind, claim.location(), message)
				.about(claim.document.clone()),
		);
	}
}

/// Reports every class that a declaration assigns but the repository config
/// does not declare.
fn undeclared_classes(
	claims: &[Claim],
	configs: &ConfigSet,
	diagnostics: &mut Vec<Diagnostic>,
) {
	for claim in claims {
		for (index, class) in claim.declaration.classes.iter().enumerate() {
			if configs.classes().contains_key(class) {
				continue;
			}
			let location =
				claim.location_of(format!("{}.classes[{index}]", claim.field));
			diagnostics.push(
				Diagnostic::new(
					DiagnosticKind::UndeclaredClass,
					location,
					format!(
						"class `{class}` is not declared. Declare it under \
						 `define.classes` in .source-sentry/source-sentry.yaml, or remove \
						 it from this declaration"
					),
				)
				.about(claim.document.clone()),
			);
		}
	}
}
