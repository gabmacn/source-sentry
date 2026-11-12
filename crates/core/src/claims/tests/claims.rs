//! Claims, their validation, tombstones, classes and case collisions.

use super::{
	BASE, REPOSITORY, assert_clean, inventory, kinds, local, lockfile, markdown,
	only, paths, sealed_with_source, tombstone, with_source,
};
use crate::claims::{
	DiagnosticKind, Inventory, LockfileKind, Mode, Presence, ResolvedSource,
	Severity,
};
use crate::model::{DocumentId, RemoteIdentity, Service};
use crate::testing::path;

/// One document in each mode, all with the source `src/a.rs`.
fn every_mode() -> Inventory {
	let local_lock = lockfile(&[("listed.pdf", &with_source("/src/a.rs"))], &[]);
	let remote_lock = lockfile(
		&[
			("abstract:privacy-video", &with_source("/src/a.rs")),
			(
				"google-drive:1a2B3c",
				r#"{"sources": [{"path": "/src/a.rs"}], "url": "https://docs.google.com/document/d/1a2B3c/edit"}"#,
			),
		],
		&[],
	);
	inventory(&[
		(
			"docs/on-file.md",
			&markdown("  sources:\n    - path: '../src/a.rs'\n"),
		),
		("docs/listed.pdf", "%PDF"),
		("docs/source-sentry.lock", &local_lock),
		(".source-sentry/remotes/legal.lock", &remote_lock),
		("src/a.rs", "fn main() {}"),
	])
}

#[test]
fn each_mode_is_recorded_with_its_site_and_identity() {
	let inventory = every_mode();
	assert_clean(&inventory);
	let sites: Vec<_> = inventory
		.claims
		.iter()
		.map(|claim| {
			(
				claim.document.clone(),
				claim.mode.clone(),
				claim.declared_in.to_string(),
				claim.field.clone(),
				claim.presence,
			)
		})
		.collect();
	let legal = || Mode::RemoteLockfile {
		owner: "legal".to_owned(),
	};
	assert_eq!(
		sites,
		[
			(
				local("docs/listed.pdf"),
				Mode::LocalLockfile,
				"docs/source-sentry.lock".to_owned(),
				r#"documents["listed.pdf"]"#.to_owned(),
				Presence::Present,
			),
			(
				local("docs/on-file.md"),
				Mode::OnFile,
				"docs/on-file.md".to_owned(),
				"source-sentry".to_owned(),
				Presence::Present,
			),
			(
				DocumentId::Remote(RemoteIdentity::Abstract(
					"privacy-video".to_owned()
				)),
				legal(),
				".source-sentry/remotes/legal.lock".to_owned(),
				r#"documents["abstract:privacy-video"]"#.to_owned(),
				Presence::Unknown,
			),
			(
				DocumentId::Remote(RemoteIdentity::Service {
					service: Service::GoogleDrive,
					file_id: "1a2B3c".to_owned(),
				}),
				legal(),
				".source-sentry/remotes/legal.lock".to_owned(),
				r#"documents["google-drive:1a2B3c"]"#.to_owned(),
				Presence::Unknown,
			),
		]
	);
}

#[test]
fn every_mode_resolves_its_sources_and_records_its_lockfile() {
	let inventory = every_mode();
	// Every source resolves to the same file, whatever the mode.
	for claim in &inventory.claims {
		let [ResolvedSource::Path(source)] = claim.sources.as_slice() else {
			panic!("expected one path source in {claim:#?}");
		};
		assert_eq!(source.path, path("src/a.rs"));
		assert!(source.exists);
		assert_eq!(source.attestation, None);
	}
	assert!(inventory.claims[3].declaration.url.is_some());

	let lockfiles: Vec<_> = inventory
		.lockfiles
		.iter()
		.map(|lockfile| (lockfile.path.to_string(), lockfile.kind.clone()))
		.collect();
	assert_eq!(
		lockfiles,
		[
			(
				".source-sentry/remotes/legal.lock".to_owned(),
				LockfileKind::Remote {
					owner: "legal".to_owned()
				}
			),
			(
				"docs/source-sentry.lock".to_owned(),
				LockfileKind::Local {
					directory: path("docs")
				}
			),
		]
	);
	assert!(inventory.lockfiles.iter().all(|lockfile| lockfile.parsed));
}

#[test]
fn only_direct_children_of_the_remotes_folder_are_owner_lockfiles() {
	let remote = lockfile(&[("abstract:video", "{}")], &[]);
	let inventory = inventory(&[
		(".source-sentry/remotes/team/legal.lock", &remote),
		(".source-sentry/remotes/.lock", &remote),
		(".source-sentry/legal.lock", &remote),
	]);
	assert_clean(&inventory);
	assert_eq!(inventory.claims, []);
	assert_eq!(inventory.lockfiles, []);
}

#[test]
fn a_document_claimed_on_file_and_in_a_lockfile_is_an_integrity_error() {
	let inventory = inventory(&[
		("docs/a.md", &markdown("  owners: ['legal']\n")),
		("docs/source-sentry.lock", &lockfile(&[("a.md", "{}")], &[])),
	]);
	let diagnostic = only(&inventory, DiagnosticKind::DoubleClaim);
	assert_eq!(diagnostic.severity(), Severity::Integrity);
	assert_eq!(diagnostic.document, Some(local("docs/a.md")));
	assert_eq!(diagnostic.location.file, path("docs/a.md"));
	assert_eq!(diagnostic.related.len(), 1);
	assert_eq!(diagnostic.related[0].file, path("docs/source-sentry.lock"));
	assert!(diagnostic.message.contains("claimed 2 times"));
	// Both claims are kept, so the report can show each.
	assert_eq!(inventory.claims_of(&local("docs/a.md")).count(), 2);
	assert!(inventory.has_errors());
}

#[test]
fn a_remote_identity_in_two_owner_lockfiles_is_a_double_claim() {
	let entry = [("abstract:video", "{}")];
	let inventory = inventory(&[
		(".source-sentry/remotes/legal.lock", &lockfile(&entry, &[])),
		(
			".source-sentry/remotes/marketing.lock",
			&lockfile(&entry, &[]),
		),
		// The namespaces never merge, so this is a different document.
		(
			".source-sentry/remotes/other.lock",
			&lockfile(&[("miro:video", "{}")], &[]),
		),
	]);
	let diagnostic = only(&inventory, DiagnosticKind::DoubleClaim);
	assert_eq!(
		diagnostic.document,
		Some(DocumentId::Remote(RemoteIdentity::Abstract(
			"video".to_owned()
		)))
	);
	assert_eq!(
		diagnostic.location.file,
		path(".source-sentry/remotes/legal.lock")
	);
	assert_eq!(
		diagnostic.related[0].file,
		path(".source-sentry/remotes/marketing.lock")
	);
}

#[test]
fn a_remote_identity_retired_in_another_owner_lockfile_is_an_error() {
	let inventory = inventory(&[
		(
			".source-sentry/remotes/legal.lock",
			&lockfile(&[("abstract:video", "{}")], &[]),
		),
		(
			".source-sentry/remotes/marketing.lock",
			&lockfile(&[], &[("abstract:video", &tombstone())]),
		),
		// Retiring and reusing an identity in one lockfile is allowed.
		(
			".source-sentry/remotes/sales.lock",
			&lockfile(&[("miro:board", "{}")], &[("miro:board", &tombstone())]),
		),
	]);
	let diagnostic = only(&inventory, DiagnosticKind::RetiredInAnotherLockfile);
	assert_eq!(diagnostic.severity(), Severity::Configuration);
	assert_eq!(
		diagnostic.location.field.as_deref(),
		Some(r#"documents["abstract:video"]"#)
	);
	assert_eq!(
		diagnostic.related[0].field.as_deref(),
		Some(r#"retired["abstract:video"]"#)
	);
	assert_eq!(inventory.tombstones.len(), 2);
}

#[test]
fn only_the_nearest_lockfile_may_list_a_document() {
	let inventory = inventory(&[
		(
			"docs/source-sentry.lock",
			&lockfile(
				&[("guide.md", "{}"), ("api/endpoints.md", "{}")],
				&[("api/old.md", &tombstone())],
			),
		),
		(
			"docs/api/source-sentry.lock",
			&lockfile(&[("auth.md", "{}")], &[]),
		),
		("docs/guide.md", "# Guide"),
		("docs/api/endpoints.md", "# Endpoints"),
		("docs/api/auth.md", "# Auth"),
	]);
	assert_eq!(
		kinds(&inventory),
		[
			DiagnosticKind::OutsideAuthority,
			DiagnosticKind::OutsideAuthority
		]
	);
	let fields: Vec<_> = inventory
		.diagnostics
		.iter()
		.map(|diagnostic| {
			assert_eq!(diagnostic.location.file, path("docs/source-sentry.lock"));
			assert_eq!(
				diagnostic.related[0].file,
				path("docs/api/source-sentry.lock")
			);
			diagnostic.location.field.clone().unwrap()
		})
		.collect();
	assert_eq!(
		fields,
		[
			r#"documents["api/endpoints.md"]"#,
			r#"retired["api/old.md"]"#
		]
	);
	assert_eq!(
		inventory
			.lockfile_for(&path("docs/api/new.md"))
			.map(|lockfile| lockfile.path.to_string()),
		Some("docs/api/source-sentry.lock".to_owned())
	);
	assert_eq!(
		inventory
			.lockfile_for(&path("docs/new.md"))
			.map(|lockfile| lockfile.path.to_string()),
		Some("docs/source-sentry.lock".to_owned())
	);
	assert!(inventory.lockfile_for(&path("src/new.md")).is_none());
}

#[test]
fn a_missing_listed_document_is_an_error_and_a_missing_record_breaks_integrity()
{
	let inventory = inventory(&[(
		"docs/source-sentry.lock",
		&lockfile(
			&[
				("draft.md", &with_source("/src/a.rs")),
				("audit.md", &sealed_with_source("/src/a.rs")),
			],
			&[],
		),
	)]);
	let found: Vec<_> = inventory
		.diagnostics
		.iter()
		.filter(|diagnostic| diagnostic.kind != DiagnosticKind::MissingSource)
		.map(|diagnostic| {
			(
				diagnostic.kind,
				diagnostic.severity(),
				diagnostic.location.field.clone(),
			)
		})
		.collect();
	assert_eq!(
		found,
		[
			(
				DiagnosticKind::MissingRecord,
				Severity::Integrity,
				Some(r#"documents["audit.md"]"#.to_owned())
			),
			(
				DiagnosticKind::MissingDocument,
				Severity::Configuration,
				Some(r#"documents["draft.md"]"#.to_owned())
			),
		]
	);
	assert!(
		inventory
			.claims
			.iter()
			.all(|claim| claim.presence == Presence::Missing)
	);
}

#[test]
fn tombstones_are_carried_and_a_reused_path_is_unclaimed_again() {
	let sealed_tombstone = format!(
		r#"{{"retired": {}, "seal": {{"at": "2026-09-01T00:00:00Z", "by": "Carla", "oid": {}}}}}"#,
		super::REVIEW,
		super::OID
	);
	let inventory = inventory(&[
		(
			"docs/source-sentry.lock",
			&lockfile(
				&[("relisted.md", "{}")],
				&[
					("audit-0001.md", &sealed_tombstone),
					("reused.md", &tombstone()),
					("relisted.md", &tombstone()),
				],
			),
		),
		("docs/reused.md", "# Back again"),
	]);
	// A deleted record with a tombstone is fine; an entry listed beside a
	// tombstone for the same path is still a document, and still missing.
	let diagnostic = only(&inventory, DiagnosticKind::MissingDocument);
	assert_eq!(
		diagnostic.location.field.as_deref(),
		Some(r#"documents["relisted.md"]"#)
	);
	let tombstones: Vec<_> = inventory
		.tombstones
		.iter()
		.map(|tombstone| {
			(
				tombstone.document.clone(),
				tombstone.field.clone(),
				tombstone.presence,
				tombstone.retired.seal.is_some(),
			)
		})
		.collect();
	assert_eq!(
		tombstones,
		[
			(
				local("docs/audit-0001.md"),
				r#"retired["audit-0001.md"]"#.to_owned(),
				Presence::Missing,
				true
			),
			(
				local("docs/relisted.md"),
				r#"retired["relisted.md"]"#.to_owned(),
				Presence::Missing,
				false
			),
			(
				local("docs/reused.md"),
				r#"retired["reused.md"]"#.to_owned(),
				Presence::Present,
				false
			),
		]
	);
	let unclaimed: Vec<_> = inventory
		.unclaimed
		.iter()
		.map(|file| file.path.clone())
		.collect();
	assert_eq!(unclaimed, paths(&["docs/reused.md"]));
}

#[test]
fn a_lockfile_that_fails_to_parse_is_a_configuration_error_with_its_line() {
	let inventory = inventory(&[
		("docs/source-sentry.lock", "{\n  \"documents\": {,\n}"),
		(
			"notes/source-sentry.lock",
			&lockfile(&[("a.md", r#"{"seal": {}}"#)], &[]),
		),
		(".source-sentry/remotes/legal.lock", r#"{"schema": 7}"#),
	]);
	let found: Vec<_> = inventory
		.diagnostics
		.iter()
		.map(|diagnostic| {
			assert_eq!(diagnostic.kind, DiagnosticKind::InvalidLockfile);
			assert_eq!(diagnostic.severity(), Severity::Configuration);
			(
				diagnostic.location.file.to_string(),
				diagnostic.location.line,
				diagnostic.location.field.clone(),
			)
		})
		.collect();
	assert_eq!(
		found,
		[
			(
				".source-sentry/remotes/legal.lock".to_owned(),
				None,
				Some("schema".to_owned())
			),
			("docs/source-sentry.lock".to_owned(), Some(2), None),
			(
				"notes/source-sentry.lock".to_owned(),
				Some(1),
				Some(r#"documents["a.md"].seal"#.to_owned())
			),
		]
	);
	assert_eq!(inventory.claims, []);
	assert!(inventory.lockfiles.iter().all(|lockfile| !lockfile.parsed));
}

#[test]
fn a_declaration_that_fails_to_read_is_a_configuration_error_with_its_field() {
	let inventory = inventory(&[
		("docs/a.md", &markdown("  sources:\n    - pattern: *.md\n")),
		(
			"docs/b.md",
			&markdown(
				"  seal:\n    at: 2026-11-07T00:00:00Z\n    by: null\n    oid: \
				 'sha1:da39a3ee5e6b4b0d3255bfef95601890afd80709'\n",
			),
		),
	]);
	let found: Vec<_> = inventory
		.diagnostics
		.iter()
		.map(|diagnostic| {
			assert_eq!(diagnostic.kind, DiagnosticKind::InvalidDeclaration);
			assert_eq!(
				diagnostic.document,
				Some(local(&diagnostic.location.file.to_string()))
			);
			(
				diagnostic.location.file.to_string(),
				diagnostic.location.line,
				diagnostic.location.field.clone(),
			)
		})
		.collect();
	assert_eq!(
		found,
		[
			(
				"docs/a.md".to_owned(),
				Some(5),
				Some("source-sentry.sources[0]".to_owned())
			),
			(
				"docs/b.md".to_owned(),
				None,
				Some("source-sentry.seal".to_owned())
			),
		]
	);
	assert_eq!(inventory.claims, []);
}

#[test]
fn an_ignored_files_declaration_is_still_read_and_validated() {
	let inventory = inventory(&[
		(
			REPOSITORY,
			&format!("{BASE}documents:\n  ignore: ['scratch/']\n"),
		),
		("source-sentry.lock", &lockfile(&[], &[])),
		("scratch/valid.md", &markdown("  owners: ['platform']\n")),
		("scratch/invalid.md", &markdown("  owners: 'platform'\n")),
		("scratch/plain.md", "# Nothing declared"),
	]);
	let diagnostic = only(&inventory, DiagnosticKind::InvalidDeclaration);
	assert_eq!(diagnostic.location.file, path("scratch/invalid.md"));
	let claimed: Vec<_> = inventory
		.claims
		.iter()
		.map(|claim| claim.document.clone())
		.collect();
	assert_eq!(claimed, [local("scratch/valid.md")]);
	// Ignored files are never unclaimed.
	assert_eq!(inventory.unclaimed, []);
}

#[test]
fn source_sentry_files_are_never_read_for_declarations() {
	let inventory = inventory(&[(
		".source-sentry/presets/README.md",
		&markdown("  owners: 'not a list'\n"),
	)]);
	assert_clean(&inventory);
	assert_eq!(inventory.claims, []);
}

#[test]
fn a_class_the_repository_config_does_not_declare_is_a_configuration_error() {
	let inventory = inventory(&[
		(REPOSITORY, BASE),
		("docs/a.md", &markdown("  classes: ['privacy', 'legal']\n")),
		(
			".source-sentry/remotes/legal.lock",
			&lockfile(&[("abstract:video", r#"{"classes": ["secret"]}"#)], &[]),
		),
	]);
	let found: Vec<_> = inventory
		.diagnostics
		.iter()
		.map(|diagnostic| {
			assert_eq!(diagnostic.kind, DiagnosticKind::UndeclaredClass);
			diagnostic.location.field.clone().unwrap()
		})
		.collect();
	assert_eq!(
		found,
		[
			r#"documents["abstract:video"].classes[0]"#,
			"source-sentry.classes[1]"
		]
	);
	assert!(inventory.diagnostics[1].message.contains("`legal`"));
}

#[test]
fn paths_that_differ_only_in_case_are_a_warning() {
	let inventory = inventory(&[
		("docs/README.md", "a"),
		("docs/Readme.md", "b"),
		("Guides/intro.md", "c"),
		("guides/intro.md", "d"),
		("guides/other.md", "e"),
	]);
	let found: Vec<_> = inventory
		.diagnostics
		.iter()
		.map(|diagnostic| {
			assert_eq!(diagnostic.kind, DiagnosticKind::CaseCollision);
			assert_eq!(diagnostic.severity(), Severity::Warning);
			let mut files = vec![diagnostic.location.file.to_string()];
			files.extend(
				diagnostic
					.related
					.iter()
					.map(|location| location.file.to_string()),
			);
			files
		})
		.collect();
	// The colliding directories are reported once, not every file in them.
	assert_eq!(
		found,
		[
			vec!["Guides".to_owned(), "guides".to_owned()],
			vec!["docs/README.md".to_owned(), "docs/Readme.md".to_owned()],
		]
	);
	assert!(!inventory.has_errors());
}

/// The lookups keep their own copy of the key, so the key may be a
/// temporary that is gone before the iterator is used.
#[test]
fn lookups_accept_a_temporary_key() {
	let inventory = inventory(&[
		(REPOSITORY, BASE),
		("src/a.rs", ""),
		("docs/a.md", ""),
		("docs/b.md", ""),
		(
			"docs/source-sentry.lock",
			&lockfile(
				&[
					("a.md", &with_source("b.md")),
					("b.md", &with_source("/src/a.rs")),
				],
				&[],
			),
		),
	]);
	assert_clean(&inventory);
	let claims = inventory.claims_of(&local("docs/a.md"));
	let upstreams = inventory.upstreams_of(&local("docs/a.md"));
	assert_eq!(claims.count(), 1);
	assert_eq!(upstreams.collect::<Vec<_>>(), [&path("docs/b.md")]);
}
