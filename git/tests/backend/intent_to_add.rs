//! Files that a plan creates are marked intent-to-add.

use source_sentry_core::ports::{
	Canonical, PlannedDelete, PlannedWrite, Snapshot as _, WritePlan,
};
use source_sentry_git::{ApplyError, Unwritable};

use crate::support::{TestRepo, path};

/// A write that creates `file` with `contents`.
fn create(file: &str, contents: &str) -> PlannedWrite {
	PlannedWrite {
		path: path(file),
		expected: None,
		contents: contents.as_bytes().to_vec(),
	}
}

/// The canonical oid of a file in a fresh working-tree snapshot.
fn snapshot_oid(repo: &TestRepo, file: &str) -> Canonical {
	let repository = repo.open();
	let snapshot = repository.working_tree_snapshot().unwrap();
	snapshot.canonical_oids(&[path(file)]).unwrap().remove(0)
}

/// The bytes of the index file.
fn index_bytes(repo: &TestRepo) -> Vec<u8> {
	std::fs::read(repo.path().join(".git").join("index")).unwrap()
}

/// Asserts that `file` is intent-to-add: in the index with the empty blob,
/// and shown by `git status` as added in the working tree only.
fn assert_intent_to_add(repo: &TestRepo, file: &str) {
	let listed = repo.git(&["ls-files", "--stage", "--", file]);
	let empty = repo.hash(b"");
	assert!(listed.contains(empty.hex()), "{file}: {listed}");
	// `.A`: unchanged in the index, added in the working tree.
	let status = repo.git(&["status", "--porcelain=v2", "--", file]);
	assert!(status.starts_with("1 .A "), "{file}: {status}");
	assert!(status.ends_with(file), "{file}: {status}");
}

#[test]
fn created_files_are_seen_by_the_next_snapshot() {
	let repo = TestRepo::init();
	repo.write("a.md", "a\n");
	repo.commit_all("add a document");

	let plan = WritePlan {
		writes: vec![
			create("source-sentry.lock", "{}\n"),
			create("docs/new/source-sentry.lock", "{\"nested\": true}\n"),
		],
		deletes: Vec::new(),
	};
	repo.open().apply(&plan).unwrap();

	assert_intent_to_add(&repo, "source-sentry.lock");
	assert_intent_to_add(&repo, "docs/new/source-sentry.lock");
	// Nothing was staged.
	assert_eq!(repo.git(&["diff", "--cached", "--name-only"]), "");

	let repository = repo.open();
	let snapshot = repository.working_tree_snapshot().unwrap();
	let paths: Vec<String> = snapshot
		.paths()
		.unwrap()
		.iter()
		.map(ToString::to_string)
		.collect();
	assert_eq!(
		paths,
		["a.md", "docs/new/source-sentry.lock", "source-sentry.lock"]
	);
	// The index holds the empty blob; the snapshot hashes the file instead.
	assert_eq!(
		snapshot_oid(&repo, "source-sentry.lock"),
		Canonical::Oid(repo.hash(b"{}\n"))
	);
	assert_eq!(
		snapshot.read(&path("docs/new/source-sentry.lock")).unwrap(),
		b"{\"nested\": true}\n"
	);
}

#[test]
fn rewrites_and_deletes_leave_the_index_alone() {
	let repo = TestRepo::init();
	repo.write("a.md", "a\n");
	repo.write("b.lock", "b\n");
	repo.write("recreated.lock", "old\n");
	repo.commit_all("add files");
	// A tracked file deleted from the working tree is recreated, not created:
	// the index already holds it.
	std::fs::remove_file(repo.path().join("recreated.lock")).unwrap();
	let before = index_bytes(&repo);

	let a = match snapshot_oid(&repo, "a.md") {
		Canonical::Oid(id) => id,
		other => panic!("{other:?}"),
	};
	let plan = WritePlan {
		writes: vec![
			PlannedWrite {
				path: path("a.md"),
				expected: Some(a),
				contents: b"new a\n".to_vec(),
			},
			create("recreated.lock", "new\n"),
		],
		deletes: vec![PlannedDelete {
			path: path("b.lock"),
			expected: repo.blob_at("HEAD", "b.lock"),
		}],
	};
	repo.open().apply(&plan).unwrap();

	assert_eq!(repo.read("a.md"), b"new a\n");
	assert_eq!(repo.read("recreated.lock"), b"new\n");
	assert!(!repo.path().join("b.lock").exists());
	assert!(before == index_bytes(&repo), "the index changed");
}

#[test]
fn a_failed_marking_changes_nothing() {
	let repo = TestRepo::init();
	repo.write("a.md", "a\n");
	repo.write("b.lock", "b\n");
	repo.commit_all("add files");
	let plan = WritePlan {
		writes: vec![
			PlannedWrite {
				path: path("a.md"),
				expected: Some(repo.blob_at("HEAD", "a.md")),
				contents: b"new a\n".to_vec(),
			},
			create("new/c.lock", "{}\n"),
		],
		deletes: vec![PlannedDelete {
			path: path("b.lock"),
			expected: repo.blob_at("HEAD", "b.lock"),
		}],
	};
	let before = index_bytes(&repo);
	// Another git process holds the index, so `git add` cannot write it.
	let lock = repo.path().join(".git").join("index.lock");
	std::fs::write(&lock, b"").unwrap();

	let result = repo.open().apply(&plan);
	std::fs::remove_file(&lock).unwrap();

	match result {
		Err(ApplyError::IntentToAddFailed { paths, .. }) => {
			assert_eq!(paths, [path("new/c.lock")]);
		}
		other => panic!("expected a failed marking, got {other:?}"),
	}
	assert_eq!(repo.read("a.md"), b"a\n");
	assert_eq!(repo.read("b.lock"), b"b\n");
	assert!(!repo.path().join("new").exists());
	assert!(before == index_bytes(&repo), "the index changed");
	assert_eq!(repo.git(&["status", "--porcelain=v1"]), "");
}

#[test]
fn ignored_new_files_are_refused() {
	let repo = TestRepo::init();
	repo.write(".gitignore", "*.lock\n");
	repo.commit_all("ignore lockfiles");

	let plan = WritePlan {
		writes: vec![create("docs/source-sentry.lock", "{}\n")],
		deletes: Vec::new(),
	};
	assert!(matches!(
		repo.open().apply(&plan),
		Err(ApplyError::Unwritable {
			reason: Unwritable::Ignored,
			..
		})
	));
	assert!(!repo.path().join("docs").exists());
}

#[test]
fn a_sparse_index_marks_files_inside_the_cone_only() {
	let repo = TestRepo::init();
	repo.write("d/inside.md", "inside\n");
	repo.write("other/outside.md", "outside\n");
	repo.commit_all("add tree");
	repo.git(&["sparse-checkout", "set", "--cone", "--sparse-index", "d"]);

	let outside = WritePlan {
		writes: vec![
			create("d/new.lock", "{}\n"),
			create("other/new.lock", "{}\n"),
		],
		deletes: Vec::new(),
	};
	assert!(matches!(
		repo.open().apply(&outside),
		Err(ApplyError::Unwritable {
			reason: Unwritable::SparseCheckout,
			..
		})
	));
	assert!(!repo.path().join("d").join("new.lock").exists());

	let inside = WritePlan {
		writes: vec![create("d/new.lock", "{}\n")],
		deletes: Vec::new(),
	};
	repo.open().apply(&inside).unwrap();
	assert_eq!(
		snapshot_oid(&repo, "d/new.lock"),
		Canonical::Oid(repo.hash(b"{}\n"))
	);
	let listed = repo.git(&["ls-files", "--", "d/new.lock"]);
	assert_eq!(listed, "d/new.lock");
}

#[test]
fn a_split_index_is_marked() {
	let repo = TestRepo::init();
	repo.write("a.md", "a\n");
	repo.commit_all("add a document");
	repo.git(&["update-index", "--split-index"]);

	let plan = WritePlan {
		writes: vec![create("source-sentry.lock", "{}\n")],
		deletes: Vec::new(),
	};
	repo.open().apply(&plan).unwrap();

	assert_intent_to_add(&repo, "source-sentry.lock");
	assert_eq!(
		snapshot_oid(&repo, "source-sentry.lock"),
		Canonical::Oid(repo.hash(b"{}\n"))
	);
}

#[test]
fn sha256_repositories_are_marked() {
	let repo = TestRepo::init_with_format("sha256");
	repo.write("a.md", "a\n");
	repo.commit_all("add a document");

	let plan = WritePlan {
		writes: vec![create("source-sentry.lock", "{}\n")],
		deletes: Vec::new(),
	};
	repo.open().apply(&plan).unwrap();

	// The empty blob of a SHA-256 repository.
	assert_intent_to_add(&repo, "source-sentry.lock");
	let expected = repo.hash(b"{}\n");
	assert_eq!(expected.hex().len(), 64);
	assert_eq!(
		snapshot_oid(&repo, "source-sentry.lock"),
		Canonical::Oid(expected)
	);
}
