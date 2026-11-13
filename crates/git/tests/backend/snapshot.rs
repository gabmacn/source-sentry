//! Snapshot membership and content: the working tree and `--rev`.

use std::error::Error as _;

use source_sentry_core::ports::{
	BackendErrorKind, Canonical, History as _, RepoPath, Revision, Snapshot,
};
use source_sentry_git::GitRepository;

use crate::support::{TestRepo, path};

/// The paths of a snapshot, as text.
fn names(paths: &[RepoPath]) -> Vec<&str> {
	paths.iter().map(RepoPath::as_str).collect()
}

#[test]
fn the_working_tree_lists_tracked_files_only() {
	let repo = TestRepo::init();
	repo.write("b.txt", "b\n");
	repo.write("a/z.txt", "z\n");
	repo.write("a.txt", "a\n");
	let head = repo.commit_all("add files");
	repo.write("untracked.txt", "untracked\n");

	let repository = repo.open();
	let snapshot = repository.working_tree_snapshot().unwrap();
	assert_eq!(
		snapshot.revision(),
		&Revision::WorkingTree { head: Some(head) }
	);
	assert_eq!(
		names(&snapshot.paths().unwrap()),
		["a.txt", "a/z.txt", "b.txt"]
	);
}

#[test]
fn an_unborn_head_has_no_commit() {
	let repo = TestRepo::init();
	repo.write("staged.txt", "staged\n");
	repo.git(&["add", "staged.txt"]);

	let repository = repo.open();
	assert_eq!(repository.head().unwrap(), None);
	let snapshot = repository.working_tree_snapshot().unwrap();
	assert_eq!(snapshot.revision(), &Revision::WorkingTree { head: None });
	assert_eq!(names(&snapshot.paths().unwrap()), ["staged.txt"]);
	assert_eq!(
		snapshot.canonical_oids(&[path("staged.txt")]).unwrap(),
		[Canonical::Oid(repo.hash(b"staged\n"))]
	);
}

#[test]
fn an_unstaged_deletion_is_absent() {
	let repo = TestRepo::init();
	repo.write("kept.txt", "kept\n");
	repo.write("deleted.txt", "deleted\n");
	repo.commit_all("add files");
	std::fs::remove_file(repo.path().join("deleted.txt")).unwrap();

	let repository = repo.open();
	let snapshot = repository.working_tree_snapshot().unwrap();
	assert_eq!(names(&snapshot.paths().unwrap()), ["kept.txt"]);
}

#[test]
fn submodules_are_members_without_content() {
	let repo = TestRepo::init();
	repo.write(
		"file.txt", "one
",
	);
	let first = repo.commit_all("one");
	let gitlink = format!("160000,{},vendor/lib", first.0.hex());
	repo.git(&["update-index", "--add", "--cacheinfo", &gitlink]);
	repo.git(&["commit", "--quiet", "--message", "add a submodule entry"]);

	let repository = repo.open();
	// Without its directory, the submodule is deleted from the working tree.
	let working = repository.working_tree_snapshot().unwrap();
	assert_eq!(names(&working.paths().unwrap()), ["file.txt"]);
	// An uninitialized submodule is an empty directory, as `git clone` leaves
	// it.
	std::fs::create_dir_all(repo.path().join("vendor").join("lib")).unwrap();
	assert_eq!(names(&working.paths().unwrap()), ["file.txt", "vendor/lib"]);
	assert_eq!(
		working
			.canonical_oids(&[path("vendor/lib"), path("file.txt")])
			.unwrap(),
		[
			Canonical::Submodule,
			Canonical::Oid(repo.blob_at("HEAD", "file.txt")),
		]
	);
	assert_eq!(
		working.read(&path("vendor/lib")).unwrap_err().kind,
		BackendErrorKind::Io
	);

	let committed = repository.commit_snapshot("HEAD").unwrap();
	assert_eq!(
		names(&committed.paths().unwrap()),
		["file.txt", "vendor/lib"]
	);
	assert_eq!(
		committed.canonical_oids(&[path("vendor/lib")]).unwrap(),
		[Canonical::Submodule]
	);
	assert!(committed.read(&path("vendor/lib")).is_err());
}

/// A populated submodule is one member, with no content or child members.
fn assert_populated_submodule(snapshot: &impl Snapshot, repo: &TestRepo) {
	assert_eq!(
		names(&snapshot.paths().unwrap()),
		[".gitmodules", "vendor/lib"]
	);
	assert_eq!(
		snapshot
			.canonical_oids(&[path("vendor/lib"), path(".gitmodules")])
			.unwrap(),
		[
			Canonical::Submodule,
			Canonical::Oid(repo.blob_at("HEAD", ".gitmodules")),
		]
	);
	let error = snapshot.read(&path("vendor/lib")).unwrap_err();
	assert_eq!(error.kind, BackendErrorKind::Io);
	assert_eq!(
		error.source().unwrap().to_string(),
		"vendor/lib is a submodule, which has no file content"
	);
	for child in [
		"vendor/lib/.git",
		"vendor/lib/README.md",
		"vendor/lib/src/lib.rs",
	] {
		let expected = format!("{child} is not a file of this snapshot");
		let error = snapshot.read(&path(child)).unwrap_err();
		assert_eq!(error.kind, BackendErrorKind::Io);
		assert_eq!(error.source().unwrap().to_string(), expected);
		let error = snapshot.canonical_oids(&[path(child)]).unwrap_err();
		assert_eq!(error.kind, BackendErrorKind::Io);
		assert_eq!(error.source().unwrap().to_string(), expected);
	}
}

#[test]
fn populated_submodules_are_members_of_the_index_backed_working_tree() {
	let repo = TestRepo::init();
	repo.add_populated_submodule();
	let submodule_head = repo.git(&["-C", "vendor/lib", "rev-parse", "HEAD"]);
	assert_eq!(
		repo.git(&["ls-files", "--stage", "vendor/lib"]),
		format!("160000 {submodule_head} 0\tvendor/lib")
	);
	assert_eq!(repo.git(&["ls-files"]), ".gitmodules\nvendor/lib");

	let repository = repo.open();
	let working = repository.working_tree_snapshot().unwrap();
	assert_populated_submodule(&working, &repo);

	// Membership is captured from the index, rather than inferred by walking
	// the populated directory. Removing the gitlink does not remove its files.
	repo.git(&["update-index", "--force-remove", "vendor/lib"]);
	assert_populated_submodule(&working, &repo);
	let updated = repository.working_tree_snapshot().unwrap();
	assert_eq!(names(&updated.paths().unwrap()), [".gitmodules"]);
	assert!(repo.path().join("vendor/lib/README.md").is_file());
}

#[test]
fn populated_submodules_are_commit_members_without_content() {
	let repo = TestRepo::init();
	let head = repo.add_populated_submodule();
	let repository = repo.open();
	let committed = repository.commit_snapshot("HEAD").unwrap();
	assert_eq!(committed.revision(), &Revision::Commit(head));
	assert_populated_submodule(&committed, &repo);
}

#[test]
fn populated_submodules_and_their_contents_are_not_untracked_files() {
	let repo = TestRepo::init();
	repo.add_populated_submodule();
	// The backend uses the normal environment; override any user-wide ignore
	// file so it cannot hide the parent repository's positive control.
	repo.write(".git/info/exclude", "");
	repo.git(&[
		"config",
		"core.excludesFile",
		repo
			.path()
			.join(".git/info/exclude")
			.to_str()
			.expect("UTF-8 excludes file"),
	]);
	let repository = repo.open();
	assert_eq!(repository.untracked_files().unwrap(), []);

	repo.write("vendor/lib/README.md", "modified submodule readme\n");
	repo.write(
		"vendor/lib/untracked.md",
		"untracked inside the submodule\n",
	);
	repo.write("untracked.md", "untracked in the parent repository\n");
	assert_eq!(
		repo.git(&[
			"-C",
			"vendor/lib",
			"ls-files",
			"--others",
			"--exclude-standard",
		]),
		"untracked.md"
	);
	assert_eq!(
		names(&repository.untracked_files().unwrap()),
		["untracked.md"]
	);
	assert_eq!(
		names(&repository.working_tree_snapshot().unwrap().paths().unwrap()),
		[".gitmodules", "vendor/lib"]
	);
}

/// Checks that a sparse checkout keeps `other/` through its index blobs.
fn assert_sparse_members(repo: &TestRepo) {
	assert!(!repo.path().join("other").join("h.txt").exists());
	let repository = repo.open();
	let snapshot = repository.working_tree_snapshot().unwrap();
	assert_eq!(
		names(&snapshot.paths().unwrap()),
		["d/g.txt", "other/deep/x.txt", "other/h.txt", "r.txt"]
	);
	assert_eq!(snapshot.read(&path("other/h.txt")).unwrap(), b"outside\n");
	assert_eq!(
		snapshot
			.canonical_oids(&[path("other/h.txt"), path("d/g.txt")])
			.unwrap(),
		[
			Canonical::Oid(repo.blob_at("HEAD", "other/h.txt")),
			Canonical::Oid(repo.blob_at("HEAD", "d/g.txt")),
		]
	);
}

/// A repository with one directory inside the sparse cone and one outside.
fn sparse_repo() -> TestRepo {
	let repo = TestRepo::init();
	repo.write("d/g.txt", "inside\n");
	repo.write("other/h.txt", "outside\n");
	repo.write("other/deep/x.txt", "deep\n");
	repo.write("r.txt", "root\n");
	repo.commit_all("add tree");
	repo
}

#[test]
fn sparse_checkout_keeps_index_blobs() {
	let repo = sparse_repo();
	repo.git(&["sparse-checkout", "set", "--cone", "--no-sparse-index", "d"]);
	assert_sparse_members(&repo);
}

#[test]
fn a_sparse_index_is_expanded() {
	let repo = sparse_repo();
	repo.git(&["sparse-checkout", "set", "--cone", "--sparse-index", "d"]);
	assert_eq!(repo.git(&["config", "index.sparse"]), "true");
	assert_sparse_members(&repo);
}

#[test]
fn a_commit_snapshot_reads_the_tree() {
	let repo = TestRepo::init();
	repo.write("a.txt", "one\n");
	repo.write("dir/b.txt", "bee\n");
	let first = repo.commit_all("first");
	repo.write("a.txt", "two\n");
	std::fs::remove_file(repo.path().join("dir/b.txt")).unwrap();
	repo.commit_all("second");
	repo.write("a.txt", "uncommitted\n");

	let repository = repo.open();
	let snapshot = repository.commit_snapshot("HEAD~1").unwrap();
	assert_eq!(snapshot.revision(), &Revision::Commit(first.clone()));
	assert_eq!(names(&snapshot.paths().unwrap()), ["a.txt", "dir/b.txt"]);
	assert_eq!(snapshot.read(&path("a.txt")).unwrap(), b"one\n");
	assert_eq!(
		snapshot
			.canonical_oids(&[path("dir/b.txt"), path("a.txt")])
			.unwrap(),
		[
			Canonical::Oid(repo.blob_at(first.0.hex(), "dir/b.txt")),
			Canonical::Oid(repo.blob_at(first.0.hex(), "a.txt")),
		]
	);
	assert_eq!(
		repository.blobs_at(&first, &[path("a.txt")]).unwrap(),
		[Some(repo.blob_at(first.0.hex(), "a.txt"))]
	);
	assert!(snapshot.read(&path("missing.txt")).is_err());
}

#[test]
fn an_unknown_revision_is_an_error() {
	let repo = TestRepo::init();
	repo.write("a.txt", "one\n");
	repo.commit_all("first");

	let repository = repo.open();
	let error = repository.commit_snapshot("no-such-branch").unwrap_err();
	assert_eq!(error.kind, BackendErrorKind::Git);
}

#[test]
fn opening_finds_the_repository_from_a_subdirectory() {
	let repo = TestRepo::init();
	repo.write("deep/dir/file.txt", "x\n");
	repo.commit_all("add");

	let repository =
		GitRepository::open(repo.path().join("deep").join("dir")).unwrap();
	let work_dir = repository.work_dir().unwrap().canonicalize().unwrap();
	assert_eq!(work_dir, repo.path().canonicalize().unwrap());
}

#[test]
fn opening_outside_a_repository_fails() {
	let directory = tempfile::tempdir().unwrap();
	let error = GitRepository::open(directory.path()).unwrap_err();
	assert_eq!(error.kind, BackendErrorKind::Git);
}

/// A name remembered for Source Sentry comes before git's own, until it is
/// forgotten; forgetting twice is not an error.
#[test]
fn a_remembered_reviewer_precedes_git_user_name_until_forgotten() {
	let repo = TestRepo::init();
	repo.git(&["config", "user.name", "Test Reviewer"]);
	let repository = repo.open();
	repository.remember_reviewer(Some("-A Name")).unwrap();
	assert_eq!(
		repository.default_reviewer().unwrap().as_deref(),
		Some("-A Name")
	);
	repository.remember_reviewer(None).unwrap();
	repository.remember_reviewer(None).unwrap();
	assert_eq!(
		repository.default_reviewer().unwrap().as_deref(),
		Some("Test Reviewer")
	);
}

#[test]
fn the_default_reviewer_is_git_user_name() {
	let repo = TestRepo::init();
	repo.git(&["config", "user.name", "Test Reviewer"]);

	let repository = repo.open();
	assert_eq!(
		repository.default_reviewer().unwrap().as_deref(),
		Some("Test Reviewer")
	);
}

/// Every file under `.git`, with its contents.
fn git_directory_contents(
	repo: &TestRepo,
) -> Vec<(std::path::PathBuf, Vec<u8>)> {
	let mut files = Vec::new();
	let mut pending = vec![repo.path().join(".git")];
	while let Some(directory) = pending.pop() {
		for entry in std::fs::read_dir(&directory).unwrap() {
			let entry = entry.unwrap();
			if entry.file_type().unwrap().is_dir() {
				pending.push(entry.path());
			} else {
				files.push((entry.path(), std::fs::read(entry.path()).unwrap()));
			}
		}
	}
	files.sort();
	files
}

#[test]
fn reading_a_snapshot_writes_nothing() {
	let repo = TestRepo::init();
	repo.write(
		".gitattributes",
		"*.bin filter=lfs -text\n*.dat filter=other\n",
	);
	repo.write("asset.bin", "hydrated content\n");
	repo.write("data.dat", "payload\n");
	repo.write("doc.md", "document\n");
	repo.write("notes.txt", "kept\r\ncrlf\r\n");
	repo.commit_all("add files");
	repo.write("doc.md", "edited\n");
	// `text=auto` over committed CRLF makes `git ls-files --eol` run too.
	repo.write(
		".gitattributes",
		"*.bin filter=lfs -text\n*.dat filter=other\n*.txt text=auto\n",
	);
	let before = git_directory_contents(&repo);

	let repository = repo.open();
	let snapshot = repository.working_tree_snapshot().unwrap();
	let paths = snapshot.paths().unwrap();
	let oids = snapshot.canonical_oids(&paths).unwrap();
	let notes = paths.iter().position(|path| path.as_str() == "notes.txt");
	assert_eq!(oids[notes.unwrap()], Canonical::CrlfInIndex);
	for path in &paths {
		snapshot.read(path).unwrap();
	}
	let head = repository.head().unwrap().unwrap();
	for commit in repository.first_parent_walk(&head) {
		commit.unwrap();
	}
	repository.blobs_at(&head, &paths).unwrap();
	repository.default_reviewer().unwrap();
	drop(snapshot);
	drop(repository);

	assert!(
		before == git_directory_contents(&repo),
		"the .git directory changed"
	);
}
