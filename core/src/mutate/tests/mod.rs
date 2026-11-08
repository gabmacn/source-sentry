//! Tests of the mutation commands, against in-memory snapshots read through
//! the real configs and the real discovery.

mod attest;
mod diff;
mod expectation;
mod init;
mod migrate;
mod relocate;
mod retire;
mod seal;
mod track;

use chrono::{DateTime, Utc};

use super::Expectation;
use crate::claims::{Claim, Inventory, discover};
use crate::config::ConfigSet;
use crate::model::{DocumentId, ObjectId, Review};
use crate::ports::{Snapshot, WritePlan};
use crate::testing::{FakeSnapshot, fake_oid, path};

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

/// A source file's contents.
const SOURCE: &str = "fn a() {}\n";

/// The reviewer the tests record.
const REVIEWER: &str = "Ana Ribeiro";

/// A snapshot, its configs and its inventory, as a command receives them.
struct Tree {
	snapshot: FakeSnapshot,
	configs: ConfigSet,
	inventory: Inventory,
}

impl Tree {
	/// A working tree holding `files`, given as `(path, contents)`.
	fn new(files: &[(&str, &str)]) -> Self {
		Self::of(files.iter().fold(
			FakeSnapshot::working_tree(None),
			|snapshot, (file, contents)| {
				snapshot.with_file(file, contents.as_bytes())
			},
		))
	}

	/// Loads the configs of `snapshot`, which must be valid, and discovers.
	fn of(snapshot: FakeSnapshot) -> Self {
		let configs = match ConfigSet::load(&snapshot) {
			Ok(configs) => configs,
			Err(error) => panic!("expected valid configs, got:\n{error}"),
		};
		let inventory = discover(&snapshot, &configs).unwrap();
		Self {
			snapshot,
			configs,
			inventory,
		}
	}

	/// The tree once `plan` is applied, as the backend would apply it. Each
	/// write must expect the file's current oid, or `None` for a new file,
	/// and each delete the current oid of an existing file. No path may be
	/// both written and deleted.
	fn apply(&self, plan: &WritePlan) -> Self {
		let mut snapshot = self.snapshot.clone();
		for write in &plan.writes {
			let current = self.snapshot.read(&write.path).ok();
			assert_eq!(
				write.expected,
				current.map(|content| fake_oid(&content)),
				"the expected oid of {}",
				write.path
			);
			snapshot = snapshot.with_file(write.path.as_str(), &write.contents);
		}
		for delete in &plan.deletes {
			assert!(
				plan.writes.iter().all(|write| write.path != delete.path),
				"{} is both written and deleted",
				delete.path
			);
			let current = self.snapshot.read(&delete.path).unwrap();
			assert_eq!(
				delete.expected,
				fake_oid(&current),
				"the expected oid of deleted {}",
				delete.path
			);
			snapshot = snapshot.without_file(delete.path.as_str());
		}
		Self::of(snapshot)
	}

	/// A file's contents, as text.
	fn text(&self, file: &str) -> String {
		String::from_utf8(self.snapshot.read(&path(file)).unwrap()).unwrap()
	}

	/// The only claim of `document`.
	fn claim(&self, document: &DocumentId) -> &Claim {
		let claims: Vec<&Claim> = self.inventory.claims_of(document).collect();
		assert_eq!(claims.len(), 1, "the claims of {document}: {claims:#?}");
		claims[0]
	}

	/// Asserts that discovery found nothing wrong.
	fn assert_clean(&self) {
		let fatal: Vec<_> = self
			.inventory
			.diagnostics
			.iter()
			.filter(|diagnostic| diagnostic.severity().is_fatal())
			.collect();
		assert!(fatal.is_empty(), "expected no errors, got {fatal:#?}");
	}
}

/// The instant the tests pass in, with a fraction of a second to truncate.
fn at() -> DateTime<Utc> {
	"2026-11-07T12:34:56.789Z".parse().unwrap()
}

/// [`at`], truncated to the second, as reviews record it.
fn second() -> DateTime<Utc> {
	"2026-11-07T12:34:56Z".parse().unwrap()
}

/// The review the tests record.
fn review() -> Review {
	Review {
		at: second(),
		by: Some(REVIEWER.to_owned()),
	}
}

/// The fake canonical oid of `content`.
fn oid(content: &str) -> ObjectId {
	fake_oid(content.as_bytes())
}

/// The expectation that `file` holds `content`.
fn present(file: &str, content: &str) -> Expectation {
	Expectation::Present {
		path: path(file),
		oid: oid(content),
	}
}

/// The identity of a local document.
fn local(file: &str) -> DocumentId {
	DocumentId::Local(path(file))
}

/// The identity of a remote document.
fn remote(identity: &str) -> DocumentId {
	DocumentId::Remote(identity.parse().unwrap())
}

/// A Markdown document whose frontmatter holds `declaration`, the indented
/// lines under `source-sentry:`, between a title and a body.
fn markdown(declaration: &str) -> String {
	format!(
		"---\ntitle: 'Guide'\nsource-sentry:\n{declaration}---\n# Guide\n\nBody.\n"
	)
}

/// A Markdown document with frontmatter but no declaration.
const PLAIN: &str = "---\ntitle: 'Guide'\n---\n# Guide\n\nBody.\n";

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

/// A review, in JSON.
const REVIEW: &str = r#"{"at": "2026-11-01T00:00:00Z", "by": "Bruno Costa"}"#;

/// A path source attested at `content`, in JSON.
fn attested(source: &str, content: &str) -> String {
	format!(
		r#"{{"oid": "{}", "path": "{source}", "reviewed": {REVIEW}}}"#,
		oid(content)
	)
}

/// A seal of `content`, in JSON.
fn seal_of(content: &str) -> String {
	format!(
		r#"{{"at": "2026-11-02T00:00:00Z", "by": "Carla Mendes", "oid": "{}"}}"#,
		oid(content)
	)
}

/// A repository config that defines the series `proposals`, matching
/// `proposals/v<integer>.md`, with `lifecycle`.
fn series_config(lifecycle: &str) -> String {
	format!(
		"{BASE}series:\n  proposals:\n    pattern: 'proposals/v(?P<version>\\d+)\\.md'\n    \
		 version-kind: integer\n    lifecycle: {lifecycle}\n"
	)
}
