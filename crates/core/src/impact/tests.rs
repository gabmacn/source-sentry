//! The impact query against in-memory snapshots.

use std::collections::BTreeSet;

use super::{Impact, Match, Reach, Reached, Searched, impact};
use crate::claims::discover;
use crate::config::ConfigSet;
use crate::model::DocumentId;
use crate::testing::{FakeSnapshot, path};

/// A seal, in JSON.
const SEAL: &str = r#"{"at": "2026-11-07T00:00:00Z", "by": null, "oid": "sha1:da39a3ee5e6b4b0d3255bfef95601890afd80709"}"#;

/// The answer for `inputs` over the single project of `files`.
fn query(files: &[(&str, &str)], inputs: &[&str], reach: Reach) -> Impact {
	let snapshot = files.iter().fold(
		FakeSnapshot::working_tree(None),
		|snapshot, (file, contents)| snapshot.with_file(file, contents.as_bytes()),
	);
	let configs = ConfigSet::load_project(&snapshot, &path(""))
		.unwrap_or_else(|error| panic!("expected valid configs, got:\n{error}"));
	let inventory = discover(&snapshot, &configs).unwrap();
	let inputs: BTreeSet<_> = inputs.iter().map(|input| path(input)).collect();
	impact(
		&[Searched {
			configs: &configs,
			inventory: &inventory,
		}],
		&inputs,
		reach,
	)
}

/// A lockfile in `docs/` holding `documents`, given as `(key, JSON
/// declaration)`.
fn lockfile(documents: &[(&str, &str)]) -> String {
	let entries: Vec<String> = documents
		.iter()
		.map(|(key, value)| format!("{key:?}: {value}"))
		.collect();
	format!(
		"{{\"documents\": {{{}}}, \"retired\": {{}}, \"schema\": 1}}",
		entries.join(", ")
	)
}

/// A declaration in JSON with `paths` and `patterns` as sources.
fn sources(paths: &[&str], patterns: &[&str]) -> String {
	let sources: Vec<String> = paths
		.iter()
		.map(|source| format!(r#"{{"path": "{source}"}}"#))
		.chain(
			patterns
				.iter()
				.map(|source| format!(r#"{{"pattern": "{source}"}}"#)),
		)
		.collect();
	format!(r#"{{"sources": [{}]}}"#, sources.join(", "))
}

/// Each document reached, as `(path, depth, how)`, in order.
fn reached(impact: &Impact) -> Vec<(String, usize, String)> {
	impact
		.documents
		.iter()
		.map(|document| {
			let how = match &document.reached {
				Reached::Direct(matches) => matches
					.iter()
					.map(|found| format!("{} by {}", found.input, found.source))
					.collect::<Vec<_>>()
					.join(", "),
				Reached::Via(path) => format!("via {path}"),
			};
			(document.document.to_string(), document.depth, how)
		})
		.collect()
}

fn row(document: &str, depth: usize, how: &str) -> (String, usize, String) {
	(document.to_owned(), depth, how.to_owned())
}

/// A path source matches its path, and a pattern its spelling, including a
/// file it would take that does not exist yet, and a deleted source.
#[test]
fn inputs_match_sources_as_declared() {
	let answer = query(
		&[
			(
				"docs/source-sentry.lock",
				&lockfile(&[
					("api.md", &sources(&["/src/gone.rs"], &[])),
					("guide.md", &sources(&["/src/a.rs"], &["/src/*.rs"])),
				]),
			),
			("docs/api.md", ""),
			("docs/guide.md", ""),
			("src/a.rs", ""),
		],
		&["src/a.rs", "src/new.rs", "src/gone.rs", "src/deep/b.rs"],
		Reach::Transitive,
	);
	assert_eq!(
		reached(&answer),
		[
			row("docs/api.md", 1, "src/gone.rs by /src/gone.rs"),
			row(
				"docs/guide.md",
				1,
				"src/a.rs by /src/*.rs, src/a.rs by /src/a.rs, src/gone.rs by /src/*.rs, \
				 src/new.rs by /src/*.rs"
			),
		]
	);
	let matched: Vec<(String, bool)> = answer
		.inputs
		.iter()
		.map(|input| (input.path.to_string(), input.matched))
		.collect();
	assert_eq!(
		matched,
		[
			("src/a.rs".to_owned(), true),
			("src/deep/b.rs".to_owned(), false),
			("src/gone.rs".to_owned(), true),
			("src/new.rs".to_owned(), true),
		]
	);
}

/// A document citing a reached document is reached through it, one step
/// deeper, at its least depth; only direct citations are followed with
/// [`Reach::Direct`].
#[test]
fn documents_citing_documents_are_reached_through_them() {
	let files = [
		(
			"docs/source-sentry.lock",
			lockfile(&[
				("a.md", &sources(&["/src/x.rs"], &[])),
				("b.md", &sources(&["/docs/a.md"], &[])),
				("c.md", &sources(&["/docs/b.md", "/docs/a.md"], &[])),
				("d.md", &sources(&["/docs/c.md"], &[])),
			]),
		),
		("docs/a.md", String::new()),
		("docs/b.md", String::new()),
		("docs/c.md", String::new()),
		("docs/d.md", String::new()),
		("src/x.rs", String::new()),
	];
	let files: Vec<(&str, &str)> = files
		.iter()
		.map(|(file, contents)| (*file, contents.as_str()))
		.collect();
	let answer = query(&files, &["src/x.rs"], Reach::Transitive);
	assert_eq!(
		reached(&answer),
		[
			row("docs/a.md", 1, "src/x.rs by /src/x.rs"),
			row("docs/b.md", 2, "via docs/a.md"),
			row("docs/c.md", 2, "via docs/a.md"),
			row("docs/d.md", 3, "via docs/c.md"),
		]
	);
	let answer = query(&files, &["src/x.rs"], Reach::Direct);
	assert_eq!(
		reached(&answer),
		[row("docs/a.md", 1, "src/x.rs by /src/x.rs")]
	);
	// A changed document reaches the documents that cite it, not itself.
	let answer = query(&files, &["docs/c.md"], Reach::Transitive);
	assert_eq!(
		reached(&answer),
		[row("docs/d.md", 1, "docs/c.md by /docs/c.md"),]
	);
}

/// A sealed record is listed last and marked, and nothing is reached
/// through it.
#[test]
fn sealed_records_are_listed_last_and_lead_nowhere() {
	let answer = query(
		&[
			(
				"docs/source-sentry.lock",
				&lockfile(&[
					(
						"old.md",
						&format!(
							r#"{{"seal": {SEAL}, "sources": [{{"path": "/src/x.rs"}}]}}"#
						),
					),
					("new.md", &sources(&["/src/x.rs"], &[])),
					("on-old.md", &sources(&["/docs/old.md"], &[])),
				]),
			),
			("docs/old.md", ""),
			("docs/new.md", ""),
			("docs/on-old.md", ""),
			("src/x.rs", ""),
		],
		&["src/x.rs"],
		Reach::Transitive,
	);
	assert_eq!(
		reached(&answer),
		[
			row("docs/new.md", 1, "src/x.rs by /src/x.rs"),
			row("docs/old.md", 1, "src/x.rs by /src/x.rs"),
		]
	);
	assert_eq!(
		answer
			.documents
			.iter()
			.map(|document| document.sealed)
			.collect::<Vec<_>>(),
		[false, true]
	);
}

/// A pattern declared in a directory whose name holds glob characters
/// takes that directory literally, as discovery does.
#[test]
fn a_declaring_directory_is_taken_literally() {
	let answer = query(
		&[
			(
				"d[1]/source-sentry.lock",
				&lockfile(&[("a.md", &sources(&[], &["*.rs"]))]),
			),
			("d[1]/a.md", ""),
			("d[1]/x.rs", ""),
		],
		&["d[1]/x.rs", "d1/x.rs"],
		Reach::Transitive,
	);
	assert_eq!(answer.documents.len(), 1, "{:#?}", answer.documents);
	assert_eq!(
		answer.documents[0].document,
		DocumentId::Local(path("d[1]/a.md"))
	);
	assert_eq!(
		answer.documents[0].reached,
		Reached::Direct(vec![Match {
			input: path("d[1]/x.rs"),
			source: "*.rs".to_owned(),
		}])
	);
}

/// An on-file document is found by its frontmatter, with its settings.
#[test]
fn on_file_documents_carry_their_settings() {
	let answer = query(
		&[
			(
				".source-sentry/source-sentry.yaml",
				"version: 1\ndefine:\n  classes:\n    api: {}\n",
			),
			(
				"docs/page.md",
				"---\nsource-sentry:\n  classes: [api]\n  owners: [alice]\n  sources:\n    - path: '../src/x.rs'\n---\n# Page\n",
			),
			("src/x.rs", ""),
		],
		&["src/x.rs"],
		Reach::Transitive,
	);
	assert_eq!(answer.documents.len(), 1);
	let document = &answer.documents[0];
	assert_eq!(document.mode, crate::claims::Mode::OnFile);
	assert_eq!(document.settings.owners.value, vec!["alice".to_owned()]);
	assert_eq!(
		document
			.settings
			.classes
			.value
			.iter()
			.map(crate::model::ClassName::as_str)
			.collect::<Vec<_>>(),
		["api"]
	);
	assert_eq!(
		document.reached,
		Reached::Direct(vec![Match {
			input: path("src/x.rs"),
			source: "../src/x.rs".to_owned(),
		}])
	);
}

/// A document claimed twice, sealed first and live after, is live: it is
/// listed with its live claim, the documents citing it are reached through
/// it, and only the live claim's sources reach it, as `check` judges each
/// claim and keeps the live claim's dependencies.
#[test]
fn a_live_claim_outweighs_a_sealed_one() {
	let files = [
		(
			"docs/source-sentry.lock",
			lockfile(&[
				(
					"z.md",
					&format!(
						r#"{{"seal": {SEAL}, "sources": [{{"path": "/src/a.rs"}}, {{"path": "/src/old.rs"}}]}}"#
					),
				),
				("down.md", &sources(&["/docs/z.md"], &[])),
			]),
		),
		(
			"source-sentry.lock",
			lockfile(&[("docs/z.md", &sources(&["/src/a.rs"], &[]))]),
		),
		("docs/z.md", String::new()),
		("docs/down.md", String::new()),
		("src/a.rs", String::new()),
		("src/old.rs", String::new()),
	];
	let files: Vec<(&str, &str)> = files
		.iter()
		.map(|(file, contents)| (*file, contents.as_str()))
		.collect();
	let answer = query(&files, &["src/a.rs"], Reach::Transitive);
	assert_eq!(
		reached(&answer),
		[
			row("docs/z.md", 1, "src/a.rs by /src/a.rs"),
			row("docs/down.md", 2, "via docs/z.md"),
		]
	);
	let live = &answer.documents[0];
	assert!(!live.sealed);
	assert_eq!(live.declared_in, path("source-sentry.lock"));
	assert_eq!(live.field, r#"documents["docs/z.md"]"#);
	// A file that only the sealed claim names affects no claim that can
	// change.
	let answer = query(&files, &["src/old.rs"], Reach::Transitive);
	assert_eq!(reached(&answer), []);
	assert!(!answer.inputs[0].matched);
}
