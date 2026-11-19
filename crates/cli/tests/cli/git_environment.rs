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

/// The command quoted in backticks in `text` that begins with `start`.
fn quoted<'a>(text: &'a str, start: &str) -> &'a str {
	let begin = text
		.find(&format!("`{start}"))
		.unwrap_or_else(|| panic!("no `{start}...` command in:\n{text}"))
		+ 1;
	let end = text[begin..]
		.find('`')
		.unwrap_or_else(|| panic!("unterminated command in:\n{text}"));
	&text[begin..begin + end]
}

/// Started outside the working tree that `GIT_WORK_TREE` names, relative
/// paths start at its root and displayed paths are relative to it, as git
/// takes them, so that every printed `git` and `source-sentry` command runs
/// as printed in the same place and environment.
#[test]
fn outside_the_working_tree_paths_start_at_its_root() {
	let repo = TestRepo::init();
	repo.commit_at("2026-10-01T10:00:00Z");
	// The start lies in another repository, which must stay untouched.
	let other = repo.outer().join("other");
	fs::create_dir(&other).unwrap();
	git_in(&repo, &other, &["init", "--quiet", "--initial-branch=main"]);
	let start = other.join("sub");
	fs::create_dir(&start).unwrap();
	let git_dir = repo.path().join(".git");
	let env = [
		("GIT_DIR", os(&git_dir)),
		("GIT_WORK_TREE", os(repo.path())),
	];
	let run = |args: &[&str]| repo.run_at(&start, args, &env);

	// A change prints its staging command with paths from the root.
	let tracked =
		run(&["track", "docs/guide.md", "--source", "/src/a.rs", "--yes"]);
	tracked.expect_code(0);
	let add = staging_command(&tracked);
	assert_eq!(add, "git add -- docs/source-sentry.lock");
	repo.run_shell_at(&start, &add, &env).expect_code(0);
	assert_eq!(
		repo.git(&["diff", "--cached", "--name-only"]),
		"docs/source-sentry.lock"
	);
	assert_eq!(git_in(&repo, &other, &["status", "--porcelain"]), "");
	run(&["attest", "docs/guide.md", "--yes"]).expect_code(0);
	repo.commit_at("2026-10-02T10:00:00Z");

	// Reports name paths from the root.
	repo.write("src/a.rs", "fn changed() {}\n");
	let checked = run(&["check", "--at", AT]);
	checked.expect_code(1);
	assert!(
		checked.stdout.contains("docs/guide.md"),
		"{}",
		checked.stdout
	);
	assert!(!checked.stdout.contains(".."), "{}", checked.stdout);

	// The attestation command that `diff` prints runs as printed.
	let diffed = run(&["diff", "docs/guide.md"]);
	diffed.expect_code(0);
	let attest = diffed.stdout.lines().last().unwrap();
	assert!(attest.contains("attest docs/guide.md"), "{attest}");
	repo
		.run_shell_at(&start, &format!("{attest} --yes"), &env)
		.expect_code(0);
	repo.commit_at("2026-10-03T10:00:00Z");
	run(&["check", "--at", AT]).expect_code(0);

	// The move that `rename` asks for runs as printed.
	let refused = run(&["rename", "docs/guide.md", "docs/new.md", "--yes"]);
	refused.expect_code(2);
	let moved = quoted(&refused.stderr, "git ");
	assert_eq!(moved, "git mv -- docs/guide.md docs/new.md");
	repo.run_shell_at(&start, moved, &env).expect_code(0);
	run(&["rename", "docs/guide.md", "docs/new.md", "--yes"]).expect_code(0);
	assert!(repo.exists("docs/new.md"));

	// An absolute path inside the working tree is accepted; a relative path
	// that leaves the root is refused, as git refuses it.
	let absolute = repo.path().join("docs/new.md");
	run(&["explain", absolute.to_str().unwrap(), "--at", AT]).expect_code(0);
	let outside = run(&["explain", "../new.md", "--at", AT]);
	outside.expect_code(64);
	assert!(
		outside.stderr.contains("outside the repository"),
		"{}",
		outside.stderr
	);
}

/// `GIT_WORK_TREE` alone, started inside another repository, keeps that
/// repository and takes the working tree it names, as git does.
#[test]
fn git_work_tree_alone_keeps_the_repository_found() {
	let repo = TestRepo::init();
	repo.reviewed_guide();
	let tree = repo.outer().join("tree");
	for file in ["docs/guide.md", "src/a.rs", "docs/source-sentry.lock"] {
		let target = tree.join(file);
		fs::create_dir_all(target.parent().unwrap()).unwrap();
		fs::copy(repo.path().join(file), target).unwrap();
	}
	fs::write(tree.join("src/a.rs"), "fn changed() {}\n").unwrap();
	let env = [("GIT_WORK_TREE", os(&tree))];
	let run = repo.run_at(
		&repo.path().join("src"),
		&["explain", "docs/guide.md", "--at", AT],
		&env,
	);
	run.expect_code(0);
	assert!(
		run.stdout.contains("changed in the working tree"),
		"{}",
		run.stdout
	);
}

/// Installs a `pre-commit` hook that runs `source-sentry` with `arguments`,
/// a shell fragment.
fn install_pre_commit(repo: &TestRepo, arguments: &str) {
	let hook = repo.path().join(".git/hooks/pre-commit");
	fs::create_dir_all(hook.parent().unwrap()).unwrap();
	let binary = crate::support::BINARY.replace('\\', "/");
	fs::write(&hook, format!("#!/bin/sh\n\"{binary}\" {arguments}\n")).unwrap();
	#[cfg(unix)]
	{
		use std::os::unix::fs::PermissionsExt as _;
		fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
	}
}

/// Commits with git's `commit` command and `arguments`, running the hook.
fn commit(repo: &TestRepo, arguments: &[&str]) -> Run {
	let mut args = vec!["commit", "--quiet", "-m", "change"];
	args.extend(arguments);
	repo.git_at(repo.path(), &args, &[])
}

/// In a `pre-commit` hook of a plain `git commit`, git gives the index as
/// the relative `.git/index`, which is taken from the working tree's root,
/// as git's own commands take it, also after `-C`.
#[test]
fn a_pre_commit_hook_reads_the_index_from_the_root() {
	let repo = TestRepo::init();
	repo.reviewed_guide();
	let report = repo.outer().join("report.json");
	install_pre_commit(
		&repo,
		&format!(
			"-C docs check --at {AT} --fail-on fail --format json > \"{}\"",
			report.to_str().unwrap().replace('\\', "/")
		),
	);
	repo.write("docs/notes.txt", "notes\n");
	commit(&repo, &[]).expect_code(0);
	let report = fs::read_to_string(&report).unwrap();
	assert!(report.contains("guide.md"), "{report}");
	assert!(report.contains("\"pass\""), "{report}");
}

/// `git commit <paths>` gives its hook a temporary index: the commit's
/// tracked files, not the index's. A source whose removal is staged but
/// not committed is still tracked there, so `check` passes.
#[test]
fn a_pre_commit_hook_checks_the_files_of_the_commit() {
	let repo = TestRepo::init();
	repo.reviewed_guide();
	repo.git(&["rm", "--cached", "--quiet", "src/a.rs"]);
	assert_ne!(repo.run(&["check", "--at", AT]).code, 0);
	install_pre_commit(&repo, &format!("-C docs check --at {AT} --fail-on fail"));
	repo.write("docs/notes.txt", "notes\n");
	commit(&repo, &["--", "docs/notes.txt"]).expect_code(0);
	// The index still holds the staged removal.
	assert!(
		repo
			.git(&["status", "--porcelain", "--", "src/a.rs"])
			.starts_with("D  src/a.rs")
	);
}

/// A change that creates a file is refused while git commits with a
/// temporary index, which would lose the file's intent-to-add mark, and
/// nothing is written. A change that only rewrites files is applied.
#[test]
fn a_hook_cannot_create_files_under_a_temporary_index() {
	for arguments in [&["-a"][..], &["--", "docs/notes.txt"]] {
		let repo = TestRepo::init();
		repo.reviewed_guide();
		repo.write("src/other.md", "# Other\n");
		repo.commit_at("2026-10-03T10:00:00Z");
		repo.write("docs/notes.txt", "notes\n");
		install_pre_commit(
			&repo,
			"track src/other.md --source src/a.rs --lockfile src --yes",
		);
		let refused = commit(&repo, arguments);
		assert_ne!(refused.code, 0, "{refused:?}");
		assert!(
			refused.stderr.contains("temporary index"),
			"{}",
			refused.stderr
		);
		assert!(!repo.exists("src/source-sentry.lock"));

		fs::write(repo.path().join("src/a.rs"), "fn changed() {}\n").unwrap();
		let before = repo.read("docs/source-sentry.lock");
		install_pre_commit(&repo, "attest docs/guide.md --yes");
		commit(&repo, arguments).expect_code(0);
		assert_ne!(repo.read("docs/source-sentry.lock"), before);
	}
}
