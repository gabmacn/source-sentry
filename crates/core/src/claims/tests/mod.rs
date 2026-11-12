//! Tests of discovery against in-memory snapshots and configs.

mod claims;
mod locations;
mod series;
mod sources;
mod unclaimed;

use super::{Diagnostic, DiagnosticKind, Inventory, discover};
use crate::config::ConfigSet;
use crate::model::{DocumentId, RepoPath};
use crate::testing::{FakeSnapshot, path};

/// The repository config's path.
const REPOSITORY: &str = ".source-sentry/source-sentry.yaml";

/// A repository config that declares two classes.
const BASE: &str = "\
version: 1
define:
  classes:
    internal-docs: {}
    privacy: {}
";

/// A review, in JSON.
const REVIEW: &str = r#"{"at": "2026-11-07T00:00:00Z", "by": null}"#;

/// An oid, in JSON.
const OID: &str = r#""sha1:da39a3ee5e6b4b0d3255bfef95601890afd80709""#;

/// A working tree holding `files`, given as `(path, contents)`.
fn snapshot(files: &[(&str, &str)]) -> FakeSnapshot {
	files.iter().fold(
		FakeSnapshot::working_tree(None),
		|snapshot, (file, contents)| snapshot.with_file(file, contents.as_bytes()),
	)
}

/// Loads the configs of `files`, which must be valid, and discovers.
fn inventory(files: &[(&str, &str)]) -> Inventory {
	let snapshot = snapshot(files);
	let configs = match ConfigSet::load(&snapshot) {
		Ok(configs) => configs,
		Err(error) => panic!("expected valid configs, got:\n{error}"),
	};
	discover(&snapshot, &configs).unwrap()
}

/// The kinds of every diagnostic, in order.
fn kinds(inventory: &Inventory) -> Vec<DiagnosticKind> {
	inventory
		.diagnostics
		.iter()
		.map(|diagnostic| diagnostic.kind)
		.collect()
}

/// The only diagnostic, which must be of `kind`.
fn only(inventory: &Inventory, kind: DiagnosticKind) -> &Diagnostic {
	assert_eq!(
		kinds(inventory),
		[kind],
		"expected one diagnostic, got {:#?}",
		inventory.diagnostics
	);
	&inventory.diagnostics[0]
}

/// Asserts that discovery found nothing wrong.
fn assert_clean(inventory: &Inventory) {
	assert!(
		inventory.diagnostics.is_empty(),
		"expected no diagnostics, got {:#?}",
		inventory.diagnostics
	);
}

/// A lockfile holding `documents` and `retired`, each given as
/// `(key, JSON value)`.
fn lockfile(documents: &[(&str, &str)], retired: &[(&str, &str)]) -> String {
	let section = |entries: &[(&str, &str)]| {
		entries
			.iter()
			.map(|(key, value)| format!("{key:?}: {value}"))
			.collect::<Vec<_>>()
			.join(", ")
	};
	format!(
		"{{\"documents\": {{{}}}, \"retired\": {{{}}}, \"schema\": 1}}",
		section(documents),
		section(retired)
	)
}

/// A declaration in JSON with one path source.
fn with_source(source: &str) -> String {
	format!(r#"{{"sources": [{{"path": "{source}"}}]}}"#)
}

/// A sealed declaration in JSON with one path source.
fn sealed_with_source(source: &str) -> String {
	format!(
		r#"{{"seal": {{"at": "2026-11-07T00:00:00Z", "by": null, "oid": {OID}}}, "sources": [{{"path": "{source}"}}]}}"#
	)
}

/// A tombstone in JSON, without a seal.
fn tombstone() -> String {
	format!(r#"{{"retired": {REVIEW}}}"#)
}

/// A Markdown document whose frontmatter holds `declaration`, the indented
/// lines under `source-sentry:`.
fn markdown(declaration: &str) -> String {
	format!("---\ntitle: 'A page'\nsource-sentry:\n{declaration}---\n# A page\n")
}

/// The identity of a local document.
fn local(file: &str) -> DocumentId {
	DocumentId::Local(path(file))
}

/// Paths, for comparisons.
fn paths(files: &[&str]) -> Vec<RepoPath> {
	files.iter().map(|file| path(file)).collect()
}
