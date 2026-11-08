//! History: first-parent walks, blob lookups and shallow boundaries.

use chrono::{TimeZone as _, Utc};
use source_sentry_core::ports::{
	BackendErrorKind, Commit, FirstParent, HashAlgorithm, History as _, ObjectId,
};

use crate::support::{TestRepo, commit_id, object_id, path};

/// A repository with three linear commits, returned oldest first.
fn three_commits() -> (TestRepo, Vec<String>) {
	let repo = TestRepo::init();
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

/// Walks the first-parent chain from `HEAD` to its end.
fn walk(repo: &TestRepo) -> Vec<Commit> {
	let repository = repo.open();
	let head = repository.head().unwrap().unwrap();
	repository
		.first_parent_walk(&head)
		.collect::<Result<Vec<_>, _>>()
		.expect("walk")
}

#[test]
fn a_full_clone_walks_to_the_root() {
	let (origin, ids) = three_commits();
	let clone = TestRepo::clone_from(&origin, None);

	let commits = walk(&clone);
	let walked: Vec<&str> =
		commits.iter().map(|commit| commit.id.0.hex()).collect();
	assert_eq!(walked, [ids[2].as_str(), ids[1].as_str(), ids[0].as_str()]);
	assert_eq!(
		commits[0].first_parent,
		FirstParent::Commit(commit_id(&ids[1]))
	);
	assert_eq!(commits[2].first_parent, FirstParent::Root);
}

#[test]
fn a_shallow_clone_reports_its_boundary() {
	let (origin, ids) = three_commits();
	let clone = TestRepo::clone_from(&origin, Some(1));
	assert!(clone.path().join(".git").join("shallow").exists());

	let commits = walk(&clone);
	assert_eq!(commits.len(), 1);
	assert_eq!(commits[0].id, commit_id(&ids[2]));
	assert_eq!(commits[0].first_parent, FirstParent::Shallow);
}

#[test]
fn future_dated_commits_report_their_committer_time() {
	let repo = TestRepo::init();
	repo.write("file.txt", "one\n");
	repo.git(&["add", "--all"]);
	repo.git_with_env(
		&["commit", "--quiet", "--message", "from the future"],
		&[("GIT_COMMITTER_DATE", "2099-01-01T05:00:00+05:00")],
	);

	let commits = walk(&repo);
	let expected = Utc.with_ymd_and_hms(2099, 1, 1, 0, 0, 0).unwrap();
	assert_eq!(commits[0].committer_time, expected);
}

#[test]
fn the_walk_skips_merged_branches() {
	let repo = TestRepo::init();
	repo.write("file.txt", "base\n");
	let base = repo.commit_all("base");
	repo.git(&["switch", "--quiet", "--create", "topic"]);
	repo.write("topic.txt", "topic\n");
	let topic = repo.commit_all("topic work");
	repo.git(&["switch", "--quiet", "main"]);
	repo.write("main.txt", "main\n");
	let main = repo.commit_all("main work");
	repo.git(&["merge", "--quiet", "--no-ff", "--no-edit", "topic"]);
	let merge = repo.head();

	let walked: Vec<_> =
		walk(&repo).into_iter().map(|commit| commit.id).collect();
	assert_eq!(walked, [merge, main, base]);
	assert!(!walked.contains(&topic));
}

#[test]
fn a_missing_commit_is_an_error() {
	let repo = TestRepo::init();
	repo.write("file.txt", "one\n");
	repo.commit_all("one");
	let repository = repo.open();
	let unknown = commit_id("0123456789abcdef0123456789abcdef01234567");

	let results: Vec<_> = repository.first_parent_walk(&unknown).collect();
	assert_eq!(results.len(), 1);
	let error = results.into_iter().next().unwrap().unwrap_err();
	assert_eq!(error.kind, BackendErrorKind::MissingObject);
}

#[test]
fn blobs_are_looked_up_in_one_batch() {
	let (repo, ids) = three_commits();
	let repository = repo.open();
	let first = commit_id(&ids[0]);

	let blobs = repository
		.blobs_at(
			&first,
			&[
				path("docs/notes.md"),
				path("missing.txt"),
				path("docs"),
				path("file.txt"),
			],
		)
		.unwrap();
	assert_eq!(
		blobs,
		[
			Some(repo.blob_at(&ids[0], "docs/notes.md")),
			None,
			None,
			Some(repo.blob_at(&ids[0], "file.txt")),
		]
	);
}

#[test]
fn submodules_are_not_blobs() {
	let repo = TestRepo::init();
	repo.write("file.txt", "one\n");
	let first = repo.commit_all("one");
	let gitlink = format!("160000,{},vendor/lib", first.0.hex());
	repo.git(&["update-index", "--add", "--cacheinfo", &gitlink]);
	repo.git(&["commit", "--quiet", "--message", "add a submodule entry"]);
	let repository = repo.open();
	let head = repository.head().unwrap().unwrap();

	assert_eq!(
		repository.blobs_at(&head, &[path("vendor/lib")]).unwrap(),
		[None]
	);
}

#[test]
fn blob_returns_none_for_unknown_objects() {
	let (repo, ids) = three_commits();
	let repository = repo.open();
	let known = repo.blob_at(&ids[0], "file.txt");

	assert_eq!(
		repository.blob(&known).unwrap().as_deref(),
		Some(&b"version 1\n"[..])
	);
	let unknown = object_id("0123456789abcdef0123456789abcdef01234567");
	assert_eq!(repository.blob(&unknown).unwrap(), None);
	let sha256 = ObjectId::new(HashAlgorithm::Sha256, &"ab".repeat(32)).unwrap();
	assert_eq!(repository.blob(&sha256).unwrap(), None);
	// A commit is not a blob.
	assert_eq!(repository.blob(&commit_id(&ids[0]).0).unwrap(), None);
}
