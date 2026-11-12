//! SHA-256 repositories.

use std::fmt::Write as _;

use sha2::{Digest as _, Sha256};
use source_sentry_core::ports::{
	Canonical, FirstParent, HashAlgorithm, History as _, PlannedDelete,
	PlannedWrite, Snapshot as _, WritePlan,
};

use crate::support::{TestRepo, commit_id, object_id, path};

/// A SHA-256 repository with three commits, returned with their IDs, oldest
/// first.
fn three_commits() -> (TestRepo, Vec<String>) {
	let repo = TestRepo::init_with_format("sha256");
	let mut ids = Vec::new();
	for number in 1..=3 {
		repo.write("file.txt", format!("version {number}\n"));
		repo.write("docs/notes.md", format!("notes {number}\n"));
		ids.push(
			repo
				.commit_all(&format!("commit {number}"))
				.0
				.hex()
				.to_owned(),
		);
	}
	(repo, ids)
}

#[test]
fn ids_use_sha256() {
	let (repo, ids) = three_commits();
	let repository = repo.open();

	assert_eq!(repository.algorithm(), HashAlgorithm::Sha256);
	let head = repository.head().unwrap().unwrap();
	assert_eq!(head.0.algorithm(), HashAlgorithm::Sha256);
	assert_eq!(head.0.hex(), ids[2]);
	assert_eq!(repository.resolve("HEAD~2").unwrap().0.hex(), ids[0]);
}

#[test]
fn the_working_tree_is_canonicalized() {
	let repo = TestRepo::init_with_format("sha256");
	let content = b"an LFS asset in a SHA-256 repository\n".repeat(40);
	let hex =
		Sha256::digest(&content)
			.iter()
			.fold(String::new(), |mut hex, byte| {
				write!(hex, "{byte:02x}").unwrap();
				hex
			});
	let pointer = format!(
		"version https://git-lfs.github.com/spec/v1\noid sha256:{hex}\nsize {}\n",
		content.len()
	);
	repo.write(".gitattributes", "*.bin filter=lfs -text\n");
	repo.write("hydrated.bin", &content);
	repo.write("pointer.bin", &pointer);
	repo.write("doc.md", "document\n");
	repo.commit_all("add files");
	repo.write("doc.md", "edited\n");

	let repository = repo.open();
	let snapshot = repository.working_tree_snapshot().unwrap();
	let oids = snapshot
		.canonical_oids(&[
			path("doc.md"),
			path("pointer.bin"),
			path("hydrated.bin"),
		])
		.unwrap();
	let pointer_blob = Canonical::Oid(repo.blob_at("HEAD", "pointer.bin"));
	assert_eq!(oids[0], Canonical::Oid(repo.hash(b"edited\n")));
	assert_eq!(oids[1], pointer_blob);
	// The pointer generated for hydrated content is hashed natively.
	assert_eq!(oids[2], pointer_blob);
}

#[test]
fn a_commit_snapshot_reads_the_tree() {
	let (repo, ids) = three_commits();
	let repository = repo.open();
	let snapshot = repository.commit_snapshot("HEAD~1").unwrap();

	assert_eq!(snapshot.read(&path("file.txt")).unwrap(), b"version 2\n");
	assert_eq!(
		snapshot.canonical_oids(&[path("docs/notes.md")]).unwrap(),
		[Canonical::Oid(repo.blob_at(&ids[1], "docs/notes.md"))]
	);
}

#[test]
fn history_reaches_the_root_or_the_shallow_boundary() {
	let (origin, ids) = three_commits();

	let full = TestRepo::clone_from(&origin, None);
	let repository = full.open();
	let head = repository.head().unwrap().unwrap();
	let commits: Vec<_> = repository
		.first_parent_walk(&head)
		.collect::<Result<_, _>>()
		.unwrap();
	assert_eq!(commits.len(), 3);
	assert_eq!(
		commits[0].first_parent,
		FirstParent::Commit(commit_id(&ids[1]))
	);
	assert_eq!(commits[2].first_parent, FirstParent::Root);

	let shallow = TestRepo::clone_from(&origin, Some(1));
	let repository = shallow.open();
	let head = repository.head().unwrap().unwrap();
	let commits: Vec<_> = repository
		.first_parent_walk(&head)
		.collect::<Result<_, _>>()
		.unwrap();
	assert_eq!(commits.len(), 1);
	assert_eq!(commits[0].first_parent, FirstParent::Shallow);
}

#[test]
fn blobs_are_looked_up_by_sha256_ids() {
	let (repo, ids) = three_commits();
	let repository = repo.open();
	let first = commit_id(&ids[0]);
	let known = repo.blob_at(&ids[0], "file.txt");
	assert_eq!(known.algorithm(), HashAlgorithm::Sha256);

	assert_eq!(
		repository
			.blobs_at(&first, &[path("file.txt"), path("missing.txt")])
			.unwrap(),
		[Some(known.clone()), None]
	);
	assert_eq!(
		repository.blob(&known).unwrap().as_deref(),
		Some(&b"version 1\n"[..])
	);
	assert_eq!(repository.blob(&object_id(&"ab".repeat(32))).unwrap(), None);
	// A SHA-1 ID cannot name an object of a SHA-256 repository.
	assert_eq!(repository.blob(&object_id(&"ab".repeat(20))).unwrap(), None);
}

#[test]
fn a_write_plan_is_verified_with_sha256_ids() {
	let (repo, ids) = three_commits();
	let expected = repo.blob_at(&ids[2], "docs/notes.md");
	assert_eq!(expected.algorithm(), HashAlgorithm::Sha256);

	let plan = WritePlan {
		writes: vec![PlannedWrite {
			path: path("docs/notes.md"),
			expected: Some(expected),
			contents: b"rewritten\n".to_vec(),
		}],
		deletes: Vec::new(),
	};
	repo.open().apply(&plan).unwrap();
	assert_eq!(repo.read("docs/notes.md"), b"rewritten\n");
}

#[test]
fn deletes_are_verified_with_sha256_ids() {
	let (repo, ids) = three_commits();
	let notes = repo.blob_at(&ids[2], "docs/notes.md");
	let file = repo.blob_at(&ids[2], "file.txt");
	assert_eq!(notes.algorithm(), HashAlgorithm::Sha256);

	// A stale delete changes nothing.
	let stale = WritePlan {
		writes: vec![PlannedWrite {
			path: path("file.txt"),
			expected: Some(file.clone()),
			contents: b"rewritten\n".to_vec(),
		}],
		deletes: vec![PlannedDelete {
			path: path("docs/notes.md"),
			expected: repo.blob_at(&ids[0], "docs/notes.md"),
		}],
	};
	assert!(repo.open().apply(&stale).is_err());
	assert_eq!(repo.read("file.txt"), b"version 3\n");
	assert_eq!(repo.read("docs/notes.md"), b"notes 3\n");

	// The current oids apply the plan.
	let plan = WritePlan {
		writes: stale.writes,
		deletes: vec![PlannedDelete {
			path: path("docs/notes.md"),
			expected: notes,
		}],
	};
	repo.open().apply(&plan).unwrap();
	assert_eq!(repo.read("file.txt"), b"rewritten\n");
	assert!(!repo.path().join("docs").join("notes.md").exists());
}
