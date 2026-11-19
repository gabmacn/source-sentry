//! History: first-parent walks, blob lookups and shallow boundaries.
use source_sentry_core::ports::{
	BackendErrorKind, Commit, FirstParent, HashAlgorithm, History as _, ObjectId,
	RepoPath,
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
	let expected: jiff::Timestamp = "2099-01-01T00:00:00Z".parse().unwrap();
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

#[test]
fn populated_submodules_are_not_blobs() {
	let repo = TestRepo::init();
	let head = repo.add_populated_submodule();
	let repository = repo.open();
	assert_eq!(
		repository
			.blobs_at(&head, &[path("vendor/lib"), path(".gitmodules")])
			.unwrap(),
		[None, Some(repo.blob_at("HEAD", ".gitmodules"))]
	);
	assert_eq!(
		repository.files_ever_in(&head, &path("vendor")).unwrap(),
		[]
	);
	assert_eq!(
		repository
			.files_ever_in(&head, &path("vendor/lib"))
			.unwrap(),
		[]
	);
}

#[test]
fn populated_submodule_contents_are_not_blobs_of_the_parent_repository() {
	let repo = TestRepo::init();
	let head = repo.add_populated_submodule();
	assert_eq!(
		repo
			.open()
			.blobs_at(
				&head,
				&[path("vendor/lib/README.md"), path("vendor/lib/src/lib.rs")],
			)
			.unwrap(),
		[None, None]
	);
}

/// The repository root lists the files committed directly at the root,
/// including those deleted since, and not those in subdirectories.
#[test]
fn the_files_ever_in_the_root_are_listed() {
	let repo = TestRepo::init();
	repo.write("old.md", "removed later\n");
	repo.commit_all("add old");
	repo.git(&["rm", "--quiet", "old.md"]);
	repo.write("root.md", "root document\n");
	repo.write("docs/nested.md", "nested document\n");
	let head = repo.commit_all("replace old");
	assert_eq!(
		repo.open().files_ever_in(&head, &RepoPath::ROOT).unwrap(),
		[path("old.md"), path("root.md")]
	);
}

/// A directory that no commit has is empty, not an error.
#[test]
fn a_directory_never_committed_has_no_files() {
	let repo = TestRepo::init();
	repo.write("docs/a.md", "a\n");
	let head = repo.commit_all("add docs");
	let repository = repo.open();
	assert_eq!(
		repository.files_ever_in(&head, &path("guides")).unwrap(),
		[]
	);
	assert_eq!(
		repository
			.files_ever_in(&head, &path("docs/a.md/inner"))
			.unwrap(),
		[]
	);
}

/// A name that is not UTF-8 in an older commit is passed over without an
/// error: a commit that holds one cannot be checked at all, so such a file
/// was never one that could be read. The tree is built from raw objects, so
/// that no such name is ever created on disk.
#[test]
fn a_name_that_is_not_utf8_in_an_older_commit_is_passed_over() {
	let repo = TestRepo::init();
	repo.write(".source-sentry/remotes/current.lock", "{}\n");
	repo.git(&["add", "--all"]);
	let blob = repo.git_with_input(&["hash-object", "-w", "--stdin"], b"{}\n");
	let mut listing = Vec::new();
	for name in [b"bad-\xff.lock".as_slice(), b"old.lock"] {
		listing.extend_from_slice(format!("100644 blob {blob}\t").as_bytes());
		listing.extend_from_slice(name);
		listing.push(0);
	}
	let remotes = repo.git_with_input(&["mktree", "-z"], &listing);
	let tool = repo.git_with_input(
		&["mktree"],
		format!("040000 tree {remotes}\tremotes\n").as_bytes(),
	);
	let tree = repo.git_with_input(
		&["mktree"],
		format!("040000 tree {tool}\t.source-sentry\n").as_bytes(),
	);
	let older = repo.git(&["commit-tree", &tree, "-m", "older"]);
	let current = repo.git(&["write-tree"]);
	let head = repo.git(&["commit-tree", &current, "-p", &older, "-m", "head"]);

	assert_eq!(
		repo
			.open()
			.files_ever_in(&commit_id(&head), &path(".source-sentry/remotes"))
			.unwrap(),
		[
			path(".source-sentry/remotes/current.lock"),
			path(".source-sentry/remotes/old.lock"),
		]
	);
}

/// A tree on the way to the directory that the object database lacks is an
/// error, never an absent directory.
#[test]
fn a_missing_intermediate_tree_is_an_error() {
	let repo = TestRepo::init();
	repo.write("docs/archive/old.md", "historical document\n");
	let head = repo.commit_all("add the archive");
	let tree = repo.git(&["rev-parse", "HEAD:docs"]);
	let object = repo
		.path()
		.join(".git/objects")
		.join(&tree[..2])
		.join(&tree[2..]);
	std::fs::remove_file(object).unwrap();

	let result = repo.open().files_ever_in(&head, &path("docs/archive"));
	let error = result.expect_err("a missing tree must fail");
	assert!(
		matches!(
			error.kind,
			BackendErrorKind::Git | BackendErrorKind::MissingObject
		),
		"{error:?}"
	);
}

/// The commits that changed a path are listed newest first, from the commit
/// asked for onwards, and the list says when it was cut.
#[test]
fn the_commits_that_changed_a_path_are_listed_newest_first() {
	let (repo, ids) = three_commits();
	repo.write("other.txt", "elsewhere\n");
	repo.commit_all("commit 4");
	let repository = repo.open();
	let file = path("file.txt");

	let all = repository.path_commits(&file, None, 10).unwrap();
	let subjects: Vec<_> = all
		.commits
		.iter()
		.map(|commit| commit.subject.as_str())
		.collect();
	assert_eq!(subjects, ["commit 3", "commit 2", "commit 1"]);
	assert!(!all.more);
	assert_eq!(all.commits[0].id, ids[2]);
	assert_eq!(all.commits[0].date.len(), "2026-01-01".len());
	assert_ne!(all.commits[0].author, "");

	// From the second commit: it and what followed, not the first.
	let since = repository
		.path_commits(&file, Some(&commit_id(&ids[1])), 10)
		.unwrap();
	let listed: Vec<_> = since
		.commits
		.iter()
		.map(|commit| commit.id.as_str())
		.collect();
	assert_eq!(listed, [ids[2].as_str(), ids[1].as_str()]);

	// The first commit has no parent, so everything is listed; a limit cuts
	// the list and says so.
	let cut = repository
		.path_commits(&file, Some(&commit_id(&ids[0])), 2)
		.unwrap();
	assert_eq!((cut.commits.len(), cut.more), (2, true));
}

/// An author name may hold any character but NUL, including the control
/// characters a log format might use as separators; it stays apart from the
/// subject.
#[test]
fn an_author_name_with_control_characters_keeps_its_fields() {
	let repo = TestRepo::init();
	repo.write("src.txt", "source\n");
	repo.git(&["add", "--all"]);
	let author = "Alice\u{1f}Injected\u{1e}Name";
	repo.git_with_env(
		&["commit", "--quiet", "--message", "Real subject"],
		&[("GIT_AUTHOR_NAME", author)],
	);
	assert_eq!(repo.git(&["log", "-1", "--format=%an"]), author);

	let listed = repo.open().path_commits(&path("src.txt"), None, 5).unwrap();
	assert_eq!(listed.commits.len(), 1);
	assert_eq!(listed.commits[0].author, author);
	assert_eq!(listed.commits[0].subject, "Real subject");
	assert_eq!(listed.commits[0].id, repo.head().0.hex());
}

/// A path is taken literally: a name with pattern characters lists its own
/// commits, not those of the files a pattern would match.
#[test]
fn the_path_of_a_commit_list_is_never_a_pattern() {
	let repo = TestRepo::init();
	repo.write("a.txt", "a\n");
	repo.commit_all("add a");
	repo.write("[ab].txt", "odd\n");
	repo.commit_all("add the odd name");
	let repository = repo.open();
	let listed = repository
		.path_commits(&path("[ab].txt"), None, 10)
		.unwrap();
	assert_eq!(listed.commits.len(), 1);
	assert_eq!(listed.commits[0].subject, "add the odd name");
	// Before any commit there is nothing to list.
	let empty = TestRepo::init();
	assert_eq!(
		empty
			.open()
			.path_commits(&path("a.txt"), None, 10)
			.unwrap()
			.commits,
		[]
	);
}
