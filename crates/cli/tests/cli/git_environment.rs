//! Git's environment variables: the repository, working tree, index and
//! object directories they name are the ones Source Sentry uses, as git
//! itself would with the same directory and environment.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use crate::support::{AT, Run, TestRepo};

/// The global pathspec settings change how git reads a pathspec, and git
/// refuses all but one of them together with `--literal-pathspecs`, which
/// Source Sentry passes. They are a user's preference for typed commands
/// and leave Source Sentry's own git commands alone.
#[test]
fn global_pathspec_settings_leave_internal_commands_alone() {
	for variable in [
		"GIT_ICASE_PATHSPECS",
		"GIT_GLOB_PATHSPECS",
		"GIT_NOGLOB_PATHSPECS",
		"GIT_LITERAL_PATHSPECS",
	] {
		let repo = TestRepo::init();
		repo.reviewed_guide();
		let env = [(variable, OsStr::new("1"))];
		repo
			.run_at(repo.path(), &["check", "--at", AT], &env)
			.expect_code(0);
		// A new lockfile is marked intent-to-add with a literal pathspec.
		repo.write("src/other.md", "# Other\n");
		repo
			.run_at(
				repo.path(),
				&[
					"track",
					"src/other.md",
					"--source",
					"/src/a.rs",
					"--lockfile",
					"src",
					"--yes",
				],
				&env,
			)
			.expect_code(0);
		assert_eq!(
			repo.git(&["ls-files", "--", "src/source-sentry.lock"]),
			"src/source-sentry.lock",
			"{variable}"
		);
	}
}

/// `path` as an environment value.
fn os(path: &Path) -> &OsStr {
	path.as_os_str()
}

/// The staging command that a change printed after creating files.
fn staging_command(run: &Run) -> String {
	run
		.stdout
		.lines()
		.chain(run.stderr.lines())
		.map(str::trim)
		.find(|line| line.starts_with("git ") && line.contains(" add "))
		.unwrap_or_else(|| panic!("no staging command in:\n{run:?}"))
		.to_owned()
}

/// Runs git in `directory`, which must succeed.
fn git_in(repo: &TestRepo, directory: &Path, args: &[&str]) -> String {
	let run = repo.git_at(directory, args, &[]);
	run.expect_code(0);
	run.stdout.trim().to_owned()
}

/// A repository nested in the working tree, which the outer one does not
/// track, with one commit, so that searching upward from inside it finds it
/// rather than the outer one.
fn nested_repository(repo: &TestRepo, directory: &str) -> PathBuf {
	let nested = repo.path().join(directory);
	fs::create_dir_all(nested.join("docs")).unwrap();
	fs::write(nested.join("docs/inner.md"), "# Inner\n").unwrap();
	git_in(repo, &nested, &["init", "--quiet", "--initial-branch=main"]);
	git_in(repo, &nested, &["add", "."]);
	git_in(
		repo,
		&nested,
		&[
			"-c",
			"user.name=T",
			"-c",
			"user.email=t@example.com",
			"commit",
			"--quiet",
			"-m",
			"inner",
		],
	);
	nested
}

/// `GIT_DIR` and `GIT_WORK_TREE` choose the repository and working tree, as
/// they do for git, even where searching upward would find another one; the
/// staging command printed after the change reaches the same repository.
#[test]
fn git_dir_and_work_tree_choose_the_repository() {
	let repo = TestRepo::init();
	repo.commit_at("2026-10-01T10:00:00Z");
	let nested = nested_repository(&repo, "vendor/inner");
	let start = nested.join("docs");
	let git_dir = repo.path().join(".git");
	let env = [
		("GIT_DIR", os(&git_dir)),
		("GIT_WORK_TREE", os(repo.path())),
	];
	let run = repo.run_at(
		&start,
		&[
			"track",
			"../../../docs/guide.md",
			"--source",
			"/src/a.rs",
			"--yes",
		],
		&env,
	);
	run.expect_code(0);
	// The lockfile is marked intent-to-add in the outer repository's index;
	// the inner one is left alone.
	assert_eq!(
		repo.git(&["ls-files", "--", "docs/source-sentry.lock"]),
		"docs/source-sentry.lock"
	);
	assert_eq!(git_in(&repo, &nested, &["status", "--porcelain"]), "");
	// The printed staging command, run in the same place and environment,
	// stages that file in that repository.
	repo
		.run_shell_at(&start, &staging_command(&run), &env)
		.expect_code(0);
	assert_eq!(
		repo.git(&["diff", "--cached", "--name-only"]),
		"docs/source-sentry.lock"
	);
}

/// `GIT_DIR` alone makes the start directory the working tree, as in git.
#[test]
fn git_dir_alone_makes_the_start_the_working_tree() {
	let repo = TestRepo::init();
	repo.reviewed_guide();
	// A copy of the working tree, in which the source has changed.
	let copy = repo.outer().join("copy");
	for file in ["docs/guide.md", "src/a.rs", "docs/source-sentry.lock"] {
		let target = copy.join(file);
		fs::create_dir_all(target.parent().unwrap()).unwrap();
		fs::copy(repo.path().join(file), target).unwrap();
	}
	fs::write(copy.join("src/a.rs"), "fn changed() {}\n").unwrap();
	let git_dir = repo.path().join(".git");
	repo
		.run_at(&copy, &["check", "--at", AT], &[("GIT_DIR", os(&git_dir))])
		.expect_code(1);
	repo.run(&["check", "--at", AT]).expect_code(0);
}

/// A working tree that `core.worktree` names, away from the git directory,
/// is the one used.
#[test]
fn core_worktree_names_the_working_tree() {
	let repo = TestRepo::init();
	repo.reviewed_guide();
	let git_dir = repo.outer().join("meta.git");
	fs::rename(repo.path().join(".git"), &git_dir).unwrap();
	git_in(
		&repo,
		repo.outer(),
		&[
			"--git-dir",
			"meta.git",
			"config",
			"core.worktree",
			"../repo",
		],
	);
	let run = repo.run_at(
		&repo.path().join("docs"),
		&["explain", "guide.md", "--at", AT],
		&[("GIT_DIR", os(&git_dir))],
	);
	run.expect_code(0);
	assert!(run.stdout.contains("guide.md"), "{}", run.stdout);
}

/// A linked worktree is found through its `.git` file, whether searched for
/// or named by `GIT_DIR`.
#[test]
fn a_linked_worktree_is_found_through_its_git_file() {
	let repo = TestRepo::init();
	repo.reviewed_guide();
	let linked = repo.outer().join("linked");
	repo.git(&["worktree", "add", "--quiet", linked.to_str().unwrap()]);
	fs::write(linked.join("src/a.rs"), "fn changed() {}\n").unwrap();
	repo
		.run_at(&linked, &["check", "--at", AT], &[])
		.expect_code(1);
	let git_file = linked.join(".git");
	repo
		.run_at(
			&linked.join("docs"),
			&["check", "--at", AT],
			&[("GIT_DIR", os(&git_file)), ("GIT_WORK_TREE", os(&linked))],
		)
		.expect_code(1);
	repo.run(&["check", "--at", AT]).expect_code(0);
}

/// A bare repository has no working tree unless `GIT_WORK_TREE` gives it
/// one.
#[test]
fn a_bare_repository_needs_git_work_tree() {
	let repo = TestRepo::init();
	repo.reviewed_guide();
	let bare = repo.outer().join("bare.git");
	git_in(
		&repo,
		repo.outer(),
		&["clone", "--quiet", "--bare", "repo", "bare.git"],
	);
	let run = repo.run_at(&bare, &["check", "--at", AT], &[]);
	run.expect_code(4);
	assert!(run.stderr.contains("bare repository"), "{}", run.stderr);
	let tree = repo.outer().join("tree");
	fs::create_dir(&tree).unwrap();
	let env = [("GIT_DIR", os(&bare)), ("GIT_WORK_TREE", os(&tree))];
	repo
		.git_at(&tree, &["checkout", "--quiet", "--force", "main"], &env)
		.expect_code(0);
	repo
		.run_at(&tree, &["check", "--at", AT], &env)
		.expect_code(0);
	fs::write(tree.join("src/a.rs"), "fn changed() {}\n").unwrap();
	repo
		.run_at(&tree.join("src"), &["check", "--at", AT], &env)
		.expect_code(1);
}

/// Inside a git directory there is no working tree: the command is refused,
/// as git refuses one that needs it.
#[test]
fn the_git_directory_itself_is_refused() {
	let repo = TestRepo::init();
	repo.reviewed_guide();
	for run in [
		repo.run_at(&repo.path().join(".git"), &["check", "--at", AT], &[]),
		repo.run(&["-C", ".git/refs", "check", "--at", AT]),
	] {
		run.expect_code(4);
		assert!(
			run.stderr.contains("inside the git directory"),
			"{}",
			run.stderr
		);
	}
}

/// A working tree that does not exist is refused with its path.
#[test]
fn a_missing_working_tree_is_refused() {
	let repo = TestRepo::init();
	repo.reviewed_guide();
	let missing = repo.outer().join("missing");
	let run = repo.run_at(
		repo.path(),
		&["check", "--at", AT],
		&[("GIT_WORK_TREE", os(&missing))],
	);
	run.expect_code(4);
	assert!(run.stderr.contains("does not exist"), "{}", run.stderr);
	assert!(run.stderr.contains("missing"), "{}", run.stderr);
}

/// Outside any repository, git's own reason is given.
#[test]
fn outside_a_repository_git_gives_the_reason() {
	let repo = TestRepo::init();
	let outside = repo.outer().join("elsewhere");
	fs::create_dir(&outside).unwrap();
	let run = repo.run_at(
		&outside,
		&["check", "--at", AT],
		&[("GIT_CEILING_DIRECTORIES", os(repo.outer()))],
	);
	run.expect_code(4);
	assert!(
		run.stderr.contains("not a git repository"),
		"{}",
		run.stderr
	);
}

/// The project is the one git places the start directory in, however `-C`
/// spells it: with letters beyond ASCII, as an absolute path in the
/// verbatim form of Windows, or with other letter case where the
/// filesystem ignores it.
#[test]
fn the_start_directory_is_placed_by_git() {
	let repo = TestRepo::init();
	repo.write("docs/café/notes.md", "# Notes\n");
	repo.reviewed_guide();
	repo
		.run(&["-C", "docs/café", "explain", "notes.md", "--at", AT])
		.expect_code(0);
	let docs = repo.path().join("docs").canonicalize().unwrap();
	repo
		.run(&[
			"-C",
			docs.to_str().unwrap(),
			"explain",
			"guide.md",
			"--at",
			AT,
		])
		.expect_code(0);
	if cfg!(windows) {
		let run = repo.run(&["-C", "DOCS", "explain", "guide.md", "--at", AT]);
		run.expect_code(0);
		assert!(run.stdout.contains("guide.md"), "{}", run.stdout);
	}
}
