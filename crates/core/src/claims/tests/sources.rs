//! Source resolution, pattern expansion and dependencies between documents.

use std::collections::{BTreeMap, BTreeSet};

use super::{
	OID, REVIEW, assert_clean, inventory, kinds, local, lockfile, markdown, only,
	paths, sealed_with_source, with_source,
};
use crate::claims::sources::resolve;
use crate::claims::{
	Claim, Dependency, DiagnosticKind, Inventory, Mode, Presence,
	ResolvedPattern, ResolvedSource, Severity,
};
use crate::model::{Declaration, PathSource, PatternSource, SourceDecl};
use crate::testing::path;

/// Both local and remote lockfiles are forbidden, even for sealed records;
/// diagnostics retain the source's written line and document.
#[test]
fn lockfile_path_sources_are_configuration_errors() {
	for source in [
		"/source-sentry.lock",
		"../source-sentry.lock",
		"/.source-sentry/remotes/legal.lock",
	] {
		let inventory = inventory(&[(
			"docs/a.md",
			&markdown(&format!("  sources:\n    - path: '{source}'\n")),
		)]);
		let diagnostic = only(&inventory, DiagnosticKind::LockfileSource);
		assert_eq!(diagnostic.severity(), Severity::Configuration);
		assert_eq!(diagnostic.document, Some(local("docs/a.md")));
		assert_eq!(diagnostic.location.line, Some(5));
	}
	let inventory = inventory(&[
		(
			"docs/source-sentry.lock",
			&lockfile(&[("a.md", &sealed_with_source("source-sentry.lock"))], &[]),
		),
		("docs/a.md", ""),
	]);
	only(&inventory, DiagnosticKind::LockfileSource);
}

/// Lockfiles are excluded from current and recorded pattern membership;
/// configuration files remain ordinary sources.
#[test]
fn patterns_exclude_lockfiles_and_keep_configs() {
	let inventory = inventory(&[
		(
			"docs/a.md",
			&with_patterns(&["/inputs/**", "/.source-sentry/remotes/*"]),
		),
		("inputs/source-sentry.lock", &lockfile(&[], &[])),
		("inputs/source-sentry.yaml", "{}\n"),
		(".source-sentry/remotes/legal.lock", &lockfile(&[], &[])),
		(".source-sentry/remotes/legal.yaml", "{}\n"),
	]);
	assert_clean(&inventory);
	let found = patterns(&inventory, "docs/a.md");
	assert_eq!(matches(found[0]), ["inputs/source-sentry.yaml"]);
	assert_eq!(matches(found[1]), [".source-sentry/remotes/legal.yaml"]);
}

/// Old attestations of lockfiles do not remain pattern members, including
/// in sealed declarations, and a lockfile-only pattern is empty.
#[test]
fn recorded_pattern_members_also_exclude_lockfiles() {
	for sealed in [false, true] {
		let seal = if sealed {
			format!(
				r#", "seal": {{"at": "2026-11-07T00:00:00Z", "by": null, "oid": {OID}}}"#
			)
		} else {
			String::new()
		};
		let declaration = format!(
			r#"{{"sources": [{{"pattern": "/**/source-sentry.lock", "members": [{{"path": "/source-sentry.lock", "oid": {OID}, "reviewed": {REVIEW}}}]}}]{seal}}}"#
		);
		let inventory = inventory(&[
			(
				"source-sentry.lock",
				&lockfile(&[("a.md", &declaration)], &[]),
			),
			("a.md", ""),
		]);
		let found = patterns(&inventory, "a.md");
		assert!(found[0].members.is_empty());
		assert_eq!(matches(found[0]), Vec::<String>::new());
		if sealed {
			assert_clean(&inventory);
		} else {
			only(&inventory, DiagnosticKind::EmptyPattern);
		}
	}
}

/// The resolved patterns of the only claim of `document`.
fn patterns<'a>(
	inventory: &'a Inventory,
	document: &str,
) -> Vec<&'a ResolvedPattern> {
	let id = local(document);
	let claims: Vec<&Claim> = inventory.claims_of(&id).collect();
	let [claim] = claims.as_slice() else {
		panic!("expected one claim of {document}, got {claims:#?}");
	};
	claim
		.sources
		.iter()
		.filter_map(|source| match source {
			ResolvedSource::Pattern(pattern) => Some(pattern),
			ResolvedSource::Path(_) => None,
		})
		.collect()
}

/// A pattern's current matches, as sorted paths.
fn matches(pattern: &ResolvedPattern) -> Vec<String> {
	pattern
		.matches
		.iter()
		.flatten()
		.map(ToString::to_string)
		.collect()
}

/// A frontmatter declaration with one pattern source per entry of `patterns`.
fn with_patterns(patterns: &[&str]) -> String {
	let sources: Vec<String> = patterns
		.iter()
		.map(|pattern| format!("    - pattern: '{pattern}'\n"))
		.collect();
	markdown(&format!("  sources:\n{}", sources.concat()))
}

#[test]
fn a_single_star_stays_in_one_directory_and_a_double_star_descends() {
	let inventory = inventory(&[
		(
			"docs/a.md",
			&with_patterns(&["/src/*.rs", "/src/**/*.rs", "/src/**"]),
		),
		("src/lib.rs", ""),
		("src/deep/mod.rs", ""),
		("src/deep/er/x.rs", ""),
		("src/notes.txt", ""),
		("srcs/other.rs", ""),
	]);
	assert_clean(&inventory);
	let found: Vec<_> = patterns(&inventory, "docs/a.md")
		.into_iter()
		.map(|pattern| (pattern.glob.clone(), matches(pattern)))
		.collect();
	assert_eq!(
		found,
		[
			("/src/*.rs".to_owned(), vec!["src/lib.rs".to_owned()]),
			(
				"/src/**/*.rs".to_owned(),
				vec![
					"src/deep/er/x.rs".to_owned(),
					"src/deep/mod.rs".to_owned(),
					"src/lib.rs".to_owned()
				]
			),
			(
				"/src/**".to_owned(),
				vec![
					"src/deep/er/x.rs".to_owned(),
					"src/deep/mod.rs".to_owned(),
					"src/lib.rs".to_owned(),
					"src/notes.txt".to_owned()
				]
			),
		]
	);
}

#[test]
fn relative_and_rooted_patterns_resolve_from_their_bases() {
	let inventory = inventory(&[
		(
			"docs/guide/a.md",
			&with_patterns(&["*.txt", "./*.txt", "../api/*.md", "/*.md"]),
		),
		("docs/guide/notes.txt", ""),
		("docs/guide/deeper/notes.txt", ""),
		("docs/api/auth.md", ""),
		("README.md", ""),
		("notes.txt", ""),
	]);
	assert_clean(&inventory);
	let found: Vec<_> = patterns(&inventory, "docs/guide/a.md")
		.into_iter()
		.map(|pattern| (pattern.glob.clone(), matches(pattern)))
		.collect();
	assert_eq!(
		found,
		[
			(
				"/docs/guide/*.txt".to_owned(),
				vec!["docs/guide/notes.txt".to_owned()]
			),
			(
				"/docs/guide/*.txt".to_owned(),
				vec!["docs/guide/notes.txt".to_owned()]
			),
			(
				"/docs/api/*.md".to_owned(),
				vec!["docs/api/auth.md".to_owned()]
			),
			("/*.md".to_owned(), vec!["README.md".to_owned()]),
		]
	);
	// The union of current sources has no repeats.
	let claim = inventory
		.claims_of(&local("docs/guide/a.md"))
		.next()
		.unwrap();
	assert_eq!(claim.source_paths().len(), 3);
}

#[test]
fn a_pattern_keeps_its_attested_members_beside_its_current_matches() {
	let declaration = format!(
		r#"{{"sources": [{{"members": [{{"oid": {OID}, "path": "/db/0001.sql", "reviewed": {REVIEW}}}, {{"oid": {OID}, "path": "../db/0002.sql", "reviewed": {REVIEW}}}], "pattern": "/db/*.sql"}}]}}"#
	);
	let inventory = inventory(&[
		(
			"docs/source-sentry.lock",
			&lockfile(&[("policy.md", &declaration)], &[]),
		),
		("docs/policy.md", ""),
		("db/0002.sql", ""),
		("db/0003.sql", ""),
	]);
	assert_clean(&inventory);
	let [pattern] = patterns(&inventory, "docs/policy.md")[..] else {
		panic!("expected one pattern");
	};
	assert_eq!(matches(pattern), ["db/0002.sql", "db/0003.sql"]);
	let members: Vec<_> = pattern.members.keys().cloned().collect();
	assert_eq!(members, paths(&["db/0001.sql", "db/0002.sql"]));
}

#[test]
fn a_source_that_resolves_to_its_own_document_is_a_configuration_error() {
	let inventory = inventory(&[
		("docs/a.md", &markdown("  sources:\n    - path: './a.md'\n")),
		("notes/b.md", &with_patterns(&["*.md"])),
		("notes/c.md", ""),
	]);
	let found: Vec<_> = inventory
		.diagnostics
		.iter()
		.map(|diagnostic| {
			assert_eq!(diagnostic.kind, DiagnosticKind::SelfDependency);
			assert_eq!(diagnostic.severity(), Severity::Configuration);
			(
				diagnostic.location.file.to_string(),
				diagnostic.location.field.clone().unwrap(),
			)
		})
		.collect();
	assert_eq!(
		found,
		[
			(
				"docs/a.md".to_owned(),
				"source-sentry.sources[0].path".to_owned()
			),
			(
				"notes/b.md".to_owned(),
				"source-sentry.sources[0].pattern".to_owned()
			),
		]
	);
	// No dependency edge is invented for a self-dependency.
	assert_eq!(inventory.dependencies, []);
}

#[test]
fn a_pattern_without_matches_is_a_warning() {
	let inventory =
		inventory(&[("docs/a.md", &with_patterns(&["/tests/**/*.json"]))]);
	let diagnostic = only(&inventory, DiagnosticKind::EmptyPattern);
	assert_eq!(diagnostic.severity(), Severity::Warning);
	assert_eq!(
		diagnostic.location.field.as_deref(),
		Some("source-sentry.sources[0].pattern")
	);
	assert!(!inventory.has_errors());
	assert_eq!(
		patterns(&inventory, "docs/a.md")[0].matches,
		Some(BTreeSet::new())
	);
}

#[test]
fn patterns_that_cannot_be_resolved_or_compiled_are_configuration_errors() {
	let inventory = inventory(&[
		(
			"docs/a.md",
			&with_patterns(&["../../*.md", "/src/[", "/src/*/../a.rs", "C:/x/*.md"]),
		),
		(
			"docs/source-sentry.lock",
			&lockfile(
				&[("b.md", r#"{"sources": [{"pattern": "src\\*.rs"}]}"#)],
				&[],
			),
		),
		("docs/b.md", ""),
	]);
	let found: Vec<_> = inventory
		.diagnostics
		.iter()
		.map(|diagnostic| {
			assert_eq!(diagnostic.kind, DiagnosticKind::InvalidPattern);
			diagnostic.location.field.clone().unwrap()
		})
		.collect();
	assert_eq!(
		found,
		[
			"source-sentry.sources[0].pattern",
			"source-sentry.sources[1].pattern",
			"source-sentry.sources[2].pattern",
			"source-sentry.sources[3].pattern",
			r#"documents["b.md"].sources[0].pattern"#,
		]
	);
	let messages: Vec<&str> = inventory
		.diagnostics
		.iter()
		.map(|diagnostic| diagnostic.message.as_str())
		.collect();
	assert!(messages[0].contains("climbs above the repository root"));
	assert!(messages[1].contains("unclosed character class"));
	assert!(messages[2].contains("follows a wildcard"));
	assert!(messages[3].contains("filesystem path"));
	assert!(messages[4].contains("backslash"));
	assert!(
		inventory
			.claims
			.iter()
			.all(|claim| claim.sources.is_empty())
	);
}

#[test]
fn a_missing_named_source_is_an_error_except_for_a_sealed_record() {
	let sealed_pattern = format!(
		r#"{{"seal": {{"at": "2026-11-07T00:00:00Z", "by": null, "oid": {OID}}}, "sources": [{{"pattern": "/gone/*.rs"}}]}}"#
	);
	let inventory = inventory(&[
		(
			"docs/source-sentry.lock",
			&lockfile(
				&[
					("live.md", &with_source("/src/gone.rs")),
					("record.md", &sealed_with_source("/src/gone.rs")),
					("record-pattern.md", &sealed_pattern),
				],
				&[],
			),
		),
		("docs/live.md", ""),
		("docs/record.md", ""),
		("docs/record-pattern.md", ""),
	]);
	let diagnostic = only(&inventory, DiagnosticKind::MissingSource);
	assert_eq!(diagnostic.severity(), Severity::Configuration);
	assert_eq!(diagnostic.document, Some(local("docs/live.md")));
	assert_eq!(
		diagnostic.location.field.as_deref(),
		Some(r#"documents["live.md"].sources[0].path"#)
	);
	// The record's missing source is recorded but not reported, and its
	// pattern is not expanded.
	let record = inventory
		.claims_of(&local("docs/record.md"))
		.next()
		.unwrap();
	let [ResolvedSource::Path(source)] = record.sources.as_slice() else {
		panic!("expected one path source");
	};
	assert!(!source.exists);
	assert_eq!(
		patterns(&inventory, "docs/record-pattern.md")[0].matches,
		None
	);
}

#[test]
fn a_remote_documents_sources_resolve_from_the_root() {
	let declaration =
		r#"{"sources": [{"path": "/src/a.rs"}, {"pattern": "/src/*.rs"}]}"#;
	let inventory = inventory(&[
		(
			".source-sentry/remotes/legal.lock",
			&lockfile(&[("abstract:video", declaration)], &[]),
		),
		("src/a.rs", ""),
		("src/b.rs", ""),
	]);
	assert_clean(&inventory);
	let claim = &inventory.claims[0];
	let resolved: Vec<_> = claim.source_paths().into_iter().cloned().collect();
	assert_eq!(resolved, paths(&["src/a.rs", "src/b.rs"]));
}

#[test]
fn a_lockfile_tracked_document_used_as_a_source_is_an_edge() {
	let inventory = inventory(&[
		(
			"findings/source-sentry.lock",
			&lockfile(&[("results.md", &with_source("/tests/out.json"))], &[]),
		),
		("findings/results.md", ""),
		("tests/out.json", ""),
		(
			"decisions/choice.md",
			&markdown("  sources:\n    - pattern: '/findings/*.md'\n"),
		),
		(
			".source-sentry/remotes/legal.lock",
			&lockfile(
				&[("abstract:video", &with_source("/findings/results.md"))],
				&[],
			),
		),
	]);
	assert_clean(&inventory);
	let edges: Vec<_> = inventory
		.dependencies
		.iter()
		.map(|edge| (edge.dependent.to_string(), edge.upstream.to_string()))
		.collect();
	assert_eq!(
		edges,
		[
			(
				"decisions/choice.md".to_owned(),
				"findings/results.md".to_owned()
			),
			(
				"abstract:video".to_owned(),
				"findings/results.md".to_owned()
			),
		]
	);
	let upstreams: Vec<_> = inventory
		.upstreams_of(&local("decisions/choice.md"))
		.collect();
	assert_eq!(upstreams, [&path("findings/results.md")]);
}

#[test]
fn an_on_file_document_used_as_a_source_is_a_configuration_error() {
	let inventory = inventory(&[
		("findings/results.md", &markdown("  owners: ['qa']\n")),
		(
			"decisions/source-sentry.lock",
			&lockfile(&[("choice.md", &with_source("/findings/results.md"))], &[]),
		),
		("decisions/choice.md", ""),
	]);
	let diagnostic = only(&inventory, DiagnosticKind::OnFileSource);
	assert_eq!(diagnostic.document, Some(local("findings/results.md")));
	assert_eq!(diagnostic.location.file, path("findings/results.md"));
	assert_eq!(
		diagnostic.related[0].field.as_deref(),
		Some(r#"documents["choice.md"].sources[0]"#)
	);
	assert!(diagnostic.message.contains("track --from"));
	assert_eq!(inventory.dependencies, []);
}

#[test]
fn documents_that_are_sources_of_each_other_form_a_cycle() {
	let inventory = inventory(&[
		(
			"docs/source-sentry.lock",
			&lockfile(
				&[
					("a.md", &with_source("b.md")),
					("b.md", &with_source("c.md")),
					("c.md", &with_source("a.md")),
					("d.md", &with_source("a.md")),
				],
				&[],
			),
		),
		("docs/a.md", ""),
		("docs/b.md", ""),
		("docs/c.md", ""),
		("docs/d.md", ""),
	]);
	let diagnostic = only(&inventory, DiagnosticKind::DependencyCycle);
	assert_eq!(diagnostic.document, Some(local("docs/a.md")));
	assert!(
		diagnostic
			.message
			.contains("`docs/a.md` → `docs/b.md` → `docs/c.md` → `docs/a.md`"),
		"{}",
		diagnostic.message
	);
	assert_eq!(diagnostic.related.len(), 2);
	assert_eq!(inventory.dependencies.len(), 4);
	assert!(inventory.dependencies.contains(&Dependency {
		dependent: local("docs/d.md"),
		upstream: path("docs/a.md"),
	}));
}

#[test]
fn a_sealed_record_adds_no_dependency() {
	let inventory = inventory(&[
		(
			"docs/source-sentry.lock",
			&lockfile(
				&[
					("audit.md", &sealed_with_source("findings.md")),
					("findings.md", "{}"),
				],
				&[],
			),
		),
		("docs/audit.md", ""),
		("docs/findings.md", ""),
	]);
	assert_eq!(kinds(&inventory), []);
	assert_eq!(inventory.dependencies, []);
}

#[test]
fn a_source_path_that_does_not_resolve_is_a_configuration_error() {
	// The adapters reject the whole declaration while reading it.
	let inventory = inventory(&[(
		"docs/a.md",
		&markdown("  sources:\n    - path: '../../outside.rs'\n"),
	)]);
	let diagnostic = only(&inventory, DiagnosticKind::InvalidDeclaration);
	assert_eq!(diagnostic.severity(), Severity::Configuration);
	assert_eq!(
		diagnostic.location.field.as_deref(),
		Some("source-sentry.sources[0].path")
	);

	// Resolution reports the same problem for a declaration that reaches it.
	let mut claims = [Claim {
		document: local("a.md"),
		mode: Mode::OnFile,
		declared_in: path("a.md"),
		field: "source-sentry".to_owned(),
		declaration: Declaration {
			sources: vec![
				SourceDecl::Path(PathSource {
					oid: None,
					path: "../outside.rs".to_owned(),
					reviewed: None,
				}),
				SourceDecl::Pattern(PatternSource {
					members: Vec::new(),
					pattern: "/*.rs".to_owned(),
					reviewed: None,
				}),
			],
			..Declaration::default()
		},
		sources: Vec::new(),
		presence: Presence::Present,
		lines: BTreeMap::new(),
	}];
	let mut diagnostics = Vec::new();
	let tracked = BTreeSet::from([path("a.md"), path("b.rs")]);
	resolve(
		&mut claims,
		&tracked,
		&crate::model::RepoPath::ROOT,
		&mut diagnostics,
	);
	let found: Vec<_> = diagnostics
		.iter()
		.map(|diagnostic| (diagnostic.kind, diagnostic.location.field.clone()))
		.collect();
	assert_eq!(
		found,
		[(
			DiagnosticKind::InvalidSourcePath,
			Some("source-sentry.sources[0].path".to_owned())
		)]
	);
	// The unresolvable source is left out; the rest keep their index.
	let indices: Vec<_> = claims[0]
		.sources
		.iter()
		.map(ResolvedSource::index)
		.collect();
	assert_eq!(indices, [1]);
}
