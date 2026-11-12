//! `track`.

use super::{
	BASE, PLAIN, REPOSITORY, SOURCE, Tree, local, lockfile, markdown, path,
	remote,
};
use crate::claims::{DiagnosticKind, Mode};
use crate::model::{
	ClassName, Declaration, DocumentId, PathError, PathSource, PatternSource,
	SourceDecl,
};
use crate::mutate::{
	MutateError, Placement, SourceSpec, Target, TrackRequest, track,
};

/// A registration of `document` at `target` with `sources`.
fn request(
	document: DocumentId,
	target: Target,
	sources: Vec<SourceSpec>,
) -> TrackRequest {
	TrackRequest {
		document,
		target,
		placement_named: true,
		sources,
		classes: Vec::new(),
		owners: Vec::new(),
		schedule: None,
		url: None,
	}
}

/// A path source.
fn source(declared: &str) -> SourceSpec {
	SourceSpec::Path(declared.to_owned())
}

/// The lockfile target, by default.
fn nearest() -> Target {
	Target::Lockfile { directory: None }
}

/// Explicit lockfiles are refused before existence checks, while config
/// files stay valid path sources.
#[test]
fn lockfiles_cannot_be_registered_as_sources() {
	let tree = fixture(&[]);
	for declared in [
		"/source-sentry.lock",
		"source-sentry.lock",
		"/.source-sentry/remotes/legal.lock",
	] {
		let result = track(
			&tree.snapshot,
			&tree.configs,
			&tree.inventory,
			&request(local("docs/guide.md"), nearest(), vec![source(declared)]),
		);
		assert!(
			matches!(result, Err(MutateError::LockfileSource { .. })),
			"{result:?}"
		);
	}
	track(
		&tree.snapshot,
		&tree.configs,
		&tree.inventory,
		&request(
			local("docs/guide.md"),
			nearest(),
			vec![source("/.source-sentry/source-sentry.yaml")],
		),
	)
	.unwrap();
}

/// A new pattern with no permitted members is refused as empty.
#[test]
fn a_pattern_matching_only_lockfiles_is_empty() {
	let tree = fixture(&[("source-sentry.lock", &lockfile(&[], &[]))]);
	let result = track(
		&tree.snapshot,
		&tree.configs,
		&tree.inventory,
		&request(
			local("docs/guide.md"),
			nearest(),
			vec![SourceSpec::Pattern("/**/source-sentry.lock".to_owned())],
		),
	);
	assert!(
		matches!(result, Err(MutateError::EmptyPattern { .. })),
		"{result:?}"
	);
}

/// The unattested declaration of `/src/a.rs`.
fn unattested() -> Vec<SourceDecl> {
	vec![SourceDecl::Path(PathSource {
		oid: None,
		path: "/src/a.rs".to_owned(),
		reviewed: None,
	})]
}

/// A tree with a source and an untracked guide, plus `extra` files.
fn fixture(extra: &[(&str, &str)]) -> Tree {
	let mut files = vec![
		(REPOSITORY, BASE),
		("src/a.rs", SOURCE),
		("docs/guide.md", PLAIN),
	];
	files.extend_from_slice(extra);
	Tree::new(&files)
}

#[test]
fn on_file_adds_the_key_and_keeps_other_bytes() {
	let tree = fixture(&[]);
	let mut request = request(
		local("docs/guide.md"),
		Target::OnFile,
		vec![source("/src/a.rs")],
	);
	request.classes = vec![ClassName::new("privacy").unwrap()];
	let planned =
		track(&tree.snapshot, &tree.configs, &tree.inventory, &request).unwrap();
	let registered = &planned.summary.registered;
	assert_eq!(registered.placement, Placement::OnFile);
	assert!(!registered.created);
	let after = tree.apply(&planned.plan);
	after.assert_clean();
	let claim = after.claim(&local("docs/guide.md"));
	assert_eq!(claim.mode, Mode::OnFile);
	assert_eq!(claim.declaration.sources, unattested());
	assert_eq!(claim.declaration.reviewed, None);
	let text = after.text("docs/guide.md");
	assert!(
		text.starts_with("---\ntitle: 'Guide'\nsource-sentry:\n"),
		"{text}"
	);
	assert!(text.ends_with("---\n# Guide\n\nBody.\n"), "{text}");
}

#[test]
fn the_nearest_existing_lockfile_is_the_default() {
	let tree = fixture(&[("source-sentry.lock", &lockfile(&[], &[]))]);
	let planned = track(
		&tree.snapshot,
		&tree.configs,
		&tree.inventory,
		&request(local("docs/guide.md"), nearest(), vec![source("/src/a.rs")]),
	)
	.unwrap();
	let registered = &planned.summary.registered;
	assert_eq!(
		registered.placement,
		Placement::Lockfile {
			directory: path(""),
		}
	);
	assert!(!registered.created);
	assert_eq!(
		registered.declared_in.field.as_deref(),
		Some("documents[\"docs/guide.md\"]")
	);
	let after = tree.apply(&planned.plan);
	after.assert_clean();
	assert_eq!(
		after.claim(&local("docs/guide.md")).mode,
		Mode::LocalLockfile
	);
}

#[test]
fn without_a_lockfile_one_is_created_beside_the_document() {
	let tree = fixture(&[]);
	let planned = track(
		&tree.snapshot,
		&tree.configs,
		&tree.inventory,
		&request(
			local("docs/guide.md"),
			nearest(),
			vec![SourceSpec::Pattern("../src/*.rs".to_owned())],
		),
	)
	.unwrap();
	let registered = &planned.summary.registered;
	assert!(registered.created);
	assert_eq!(
		registered.placement,
		Placement::Lockfile {
			directory: path("docs"),
		}
	);
	assert_eq!(registered.matches[0].matches, [path("src/a.rs")]);
	let [write] = planned.plan.writes.as_slice() else {
		panic!("expected one write");
	};
	assert_eq!(write.path, path("docs/source-sentry.lock"));
	assert_eq!(write.expected, None);
	let after = tree.apply(&planned.plan);
	after.assert_clean();
	assert_eq!(
		after.claim(&local("docs/guide.md")).declaration.sources,
		[SourceDecl::Pattern(PatternSource {
			members: Vec::new(),
			pattern: "../src/*.rs".to_owned(),
			reviewed: None,
		})]
	);
}

#[test]
fn an_explicit_ancestor_directory_is_accepted() {
	let tree = fixture(&[]);
	let target = Target::Lockfile {
		directory: Some(path("")),
	};
	let planned = track(
		&tree.snapshot,
		&tree.configs,
		&tree.inventory,
		&request(local("docs/guide.md"), target, Vec::new()),
	)
	.unwrap();
	assert_eq!(planned.plan.writes[0].path, path("source-sentry.lock"));
	tree.apply(&planned.plan).assert_clean();
}

/// An owner lockfile is created with the identity and a `url`.
#[test]
fn a_remote_document_goes_to_its_owner_lockfile() {
	let tree = fixture(&[]);
	let mut request = request(
		remote("abstract:video"),
		Target::Remote {
			owner: "legal".to_owned(),
		},
		vec![source("/src/a.rs")],
	);
	request.url = Some("https://example.com/video".to_owned());
	let planned =
		track(&tree.snapshot, &tree.configs, &tree.inventory, &request).unwrap();
	assert!(planned.summary.registered.created);
	assert_eq!(
		planned.plan.writes[0].path,
		path(".source-sentry/remotes/legal.lock")
	);
	let after = tree.apply(&planned.plan);
	after.assert_clean();
	let claim = after.claim(&remote("abstract:video"));
	assert_eq!(
		claim.declaration,
		Declaration {
			sources: unattested(),
			url: Some("https://example.com/video".to_owned()),
			..Declaration::default()
		}
	);
}

/// A second claim would be a double claim.
#[test]
fn an_already_claimed_document_is_refused() {
	let tree =
		fixture(&[("docs/other.md", &markdown("  classes: ['privacy']\n"))]);
	let error = track(
		&tree.snapshot,
		&tree.configs,
		&tree.inventory,
		&request(local("docs/other.md"), nearest(), Vec::new()),
	)
	.unwrap_err();
	assert!(
		matches!(error, MutateError::AlreadyClaimed { .. }),
		"{error:?}"
	);
}

/// A document used as a source must be lockfile-tracked.
#[test]
fn a_source_of_another_document_cannot_go_on_file() {
	let tree = fixture(&[
		(
			"source-sentry.lock",
			&lockfile(
				&[("docs/decision.md", r#"{"sources": [{"path": "guide.md"}]}"#)],
				&[],
			),
		),
		("docs/decision.md", "# Decision\n"),
	]);
	let error = track(
		&tree.snapshot,
		&tree.configs,
		&tree.inventory,
		&request(local("docs/guide.md"), Target::OnFile, Vec::new()),
	)
	.unwrap_err();
	let MutateError::UsedAsSource { dependents, .. } = &error else {
		panic!("expected a refusal, got {error:?}");
	};
	assert_eq!(dependents, &[local("docs/decision.md")]);
	// A lockfile may track it.
	let planned = track(
		&tree.snapshot,
		&tree.configs,
		&tree.inventory,
		&request(local("docs/guide.md"), nearest(), Vec::new()),
	)
	.unwrap();
	tree.apply(&planned.plan).assert_clean();
}

#[test]
fn sources_must_resolve() {
	let tree = fixture(&[]);
	let attempt = |document: DocumentId, target: Target, sources| {
		track(
			&tree.snapshot,
			&tree.configs,
			&tree.inventory,
			&request(document, target, sources),
		)
		.unwrap_err()
	};
	let video = || remote("abstract:video");
	let legal = || Target::Remote {
		owner: "legal".to_owned(),
	};

	let error = attempt(
		local("docs/guide.md"),
		nearest(),
		vec![source("/src/gone.rs")],
	);
	assert!(
		matches!(error, MutateError::MissingSource { .. }),
		"{error:?}"
	);

	let error = attempt(
		local("docs/guide.md"),
		nearest(),
		vec![source("../../src/a.rs")],
	);
	assert!(
		matches!(
			error,
			MutateError::InvalidSource {
				error: PathError::EscapesRepository(_),
				..
			}
		),
		"{error:?}"
	);

	// A remote document's sources start with `/`.
	let error = attempt(video(), legal(), vec![source("src/a.rs")]);
	assert!(
		matches!(
			error,
			MutateError::InvalidSource {
				error: PathError::NotRooted(_),
				..
			}
		),
		"{error:?}"
	);
	let error = attempt(
		video(),
		legal(),
		vec![SourceSpec::Pattern("src/*.rs".to_owned())],
	);
	assert!(
		matches!(
			error,
			MutateError::InvalidSource {
				error: PathError::NotRooted(_),
				..
			}
		),
		"{error:?}"
	);

	let error =
		attempt(local("docs/guide.md"), nearest(), vec![source("guide.md")]);
	assert!(
		matches!(error, MutateError::SelfDependency { .. }),
		"{error:?}"
	);

	let error = attempt(
		local("docs/guide.md"),
		nearest(),
		vec![source("/src/a.rs"), source("../src/a.rs")],
	);
	assert!(
		matches!(error, MutateError::DuplicateSource { .. }),
		"{error:?}"
	);
}

/// A new pattern that matches nothing is never recorded.
#[test]
fn a_new_empty_pattern_is_refused() {
	let tree = fixture(&[]);
	let error = track(
		&tree.snapshot,
		&tree.configs,
		&tree.inventory,
		&request(
			local("docs/guide.md"),
			nearest(),
			vec![SourceSpec::Pattern("/db/*.sql".to_owned())],
		),
	)
	.unwrap_err();
	assert!(
		matches!(error, MutateError::EmptyPattern { .. }),
		"{error:?}"
	);
}

/// Only the nearest lockfile may list a document.
#[test]
fn lockfile_directories_respect_authority() {
	let tree = fixture(&[
		("source-sentry.lock", &lockfile(&[("docs/a.md", "{}")], &[])),
		("docs/a.md", "# A\n"),
		("other/b.md", "# B\n"),
	]);
	let attempt = |document: &str, directory: &str| {
		track(
			&tree.snapshot,
			&tree.configs,
			&tree.inventory,
			&request(
				local(document),
				Target::Lockfile {
					directory: Some(path(directory)),
				},
				Vec::new(),
			),
		)
		.unwrap_err()
	};
	let error = attempt("docs/guide.md", "other");
	assert!(
		matches!(error, MutateError::NotAnAncestor { .. }),
		"{error:?}"
	);
	// A new lockfile in `docs` would take `docs/a.md` from the root's.
	let error = attempt("docs/guide.md", "docs");
	let MutateError::StrandedEntries { documents, .. } = &error else {
		panic!("expected a refusal, got {error:?}");
	};
	assert_eq!(documents, &[local("docs/a.md")]);

	let nested = fixture(&[
		("source-sentry.lock", &lockfile(&[], &[])),
		("docs/source-sentry.lock", &lockfile(&[], &[])),
	]);
	let error = track(
		&nested.snapshot,
		&nested.configs,
		&nested.inventory,
		&request(
			local("docs/guide.md"),
			Target::Lockfile {
				directory: Some(path("")),
			},
			Vec::new(),
		),
	)
	.unwrap_err();
	assert!(
		matches!(error, MutateError::NearerLockfile { .. }),
		"{error:?}"
	);
}

#[test]
fn other_refusals() {
	let tree = fixture(&[("docs/spec.pdf", "%PDF-1.7\n")]);
	let attempt = |request: &TrackRequest| {
		track(&tree.snapshot, &tree.configs, &tree.inventory, request).unwrap_err()
	};

	// An opaque file cannot carry a declaration.
	let error =
		attempt(&request(local("docs/spec.pdf"), Target::OnFile, Vec::new()));
	assert!(
		matches!(error, MutateError::NotDeclarationCapable { .. }),
		"{error:?}"
	);

	let mut undeclared = request(local("docs/guide.md"), nearest(), Vec::new());
	undeclared.classes = vec![ClassName::new("marketing").unwrap()];
	let error = attempt(&undeclared);
	assert!(
		matches!(error, MutateError::UndeclaredClass { .. }),
		"{error:?}"
	);

	let mut url = request(local("docs/guide.md"), nearest(), Vec::new());
	url.url = Some("https://example.com".to_owned());
	let error = attempt(&url);
	assert!(
		matches!(error, MutateError::UrlNotRemote { .. }),
		"{error:?}"
	);

	let error = attempt(&request(
		remote("abstract:video"),
		Target::OnFile,
		Vec::new(),
	));
	assert!(
		matches!(error, MutateError::TargetMismatch { .. }),
		"{error:?}"
	);

	let error = attempt(&request(local("docs/gone.md"), nearest(), Vec::new()));
	assert!(
		matches!(error, MutateError::DocumentNotFound { .. }),
		"{error:?}"
	);

	let error = attempt(&request(local(REPOSITORY), nearest(), Vec::new()));
	assert!(
		matches!(error, MutateError::NotADocument { .. }),
		"{error:?}"
	);

	let error = attempt(&request(
		remote("abstract:video"),
		Target::Remote {
			owner: "../legal".to_owned(),
		},
		Vec::new(),
	));
	assert!(
		matches!(error, MutateError::InvalidOwnerName { .. }),
		"{error:?}"
	);
}

/// Cycles are rejected, whichever registration closes them.
#[test]
fn a_registration_that_closes_a_cycle_is_refused() {
	let tree = fixture(&[
		(
			"source-sentry.lock",
			&lockfile(
				&[("docs/a.md", r#"{"sources": [{"path": "guide.md"}]}"#)],
				&[],
			),
		),
		("docs/a.md", "# A\n"),
	]);
	let error = track(
		&tree.snapshot,
		&tree.configs,
		&tree.inventory,
		&request(local("docs/guide.md"), nearest(), vec![source("a.md")]),
	)
	.unwrap_err();
	let MutateError::WouldIntroduce { diagnostics } = &error else {
		panic!("expected a refusal, got {error:?}");
	};
	assert!(
		diagnostics
			.iter()
			.any(|diagnostic| diagnostic.kind == DiagnosticKind::DependencyCycle),
		"{diagnostics:#?}"
	);
}
