//! Commands that messages suggest run as printed, pasted into the shell the
//! user started in, including after `-C` and for names that begin with `-`.

use crate::support::TestRepo;

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

/// The recovery hint of `rename` names the paths after `--`, and repeats
/// `-C`, so that it moves the file from the directory the user is in.
#[test]
fn the_rename_hint_moves_the_file_where_it_was_run() {
	for (directory, repo_argument, old, new, expected) in [
		(
			".",
			None,
			"docs/guide.md",
			"docs/new.md",
			"git mv -- docs/guide.md docs/new.md",
		),
		(
			"src",
			Some(".."),
			"docs/guide.md",
			"docs/new.md",
			"git -C .. mv -- docs/guide.md docs/new.md",
		),
		(
			"docs",
			None,
			"./-old.md",
			"new.md",
			"git mv -- '-old.md' new.md",
		),
	] {
		let repo = TestRepo::init();
		repo.write("docs/-old.md", "# Old\n");
		let document = if old.ends_with("-old.md") {
			"docs/-old.md"
		} else {
			"docs/guide.md"
		};
		repo.run(&["track", document, "--yes"]).expect_code(0);
		repo.commit_at("2026-10-01T10:00:00Z");
		let mut args = Vec::new();
		if let Some(repo_argument) = repo_argument {
			args.extend(["-C", repo_argument]);
		}
		args.extend(["rename", old, new, "--yes"]);
		let refused = repo.run_in(directory, &args);
		refused.expect_code(2);
		let command = quoted(&refused.stderr, "git ");
		assert_eq!(command, expected, "{}", refused.stderr);

		repo.run_shell_in(directory, command).expect_code(0);
		repo.run_in(directory, &args).expect_code(0);
		assert!(repo.exists("docs/new.md"), "{document} was not moved");
		assert!(!repo.exists(document), "{document} still exists");
	}
}

/// Sealing an on-file document is refused with the complete `track`
/// command that moves it into the lockfile that would hold it: a new one
/// in its directory, or the nearest existing one.
#[test]
fn the_on_file_seal_refusal_suggests_a_complete_move() {
	for (existing, expected) in [
		(
			None,
			"source-sentry -C .. track docs/guide.md --from on-file --to \
			 lockfile:docs",
		),
		(
			Some("src/a.rs"),
			"source-sentry -C .. track docs/guide.md --from on-file --to \
			 lockfile:.",
		),
	] {
		let repo = TestRepo::init();
		if let Some(existing) = existing {
			repo
				.run(&["track", existing, "--lockfile", ".", "--yes"])
				.expect_code(0);
		}
		repo
			.run(&["track", "docs/guide.md", "--on-file", "--yes"])
			.expect_code(0);
		repo.commit_at("2026-10-01T10:00:00Z");
		let seal = ["-C", "..", "seal", "docs/guide.md", "--yes"];
		let refused = repo.run_in("src", &seal);
		refused.expect_code(2);
		let command = quoted(&refused.stderr, "source-sentry ");
		assert_eq!(command, expected, "{}", refused.stderr);

		repo
			.run_shell_in("src", &format!("{command} --yes"))
			.expect_code(0);
		repo.commit_at("2026-10-02T10:00:00Z");
		repo.run_in("src", &seal).expect_code(0);
	}
}

/// The advice for a file that the index holds with CRLF line endings
/// renormalizes that file, also when it is outside the current directory
/// and when `-C` was given.
#[test]
fn the_renormalize_advice_names_the_file() {
	for (directory, repo_argument, expected) in [
		("docs", None, "git add --renormalize -- ../src/a.rs"),
		(
			"src",
			Some("../docs"),
			"git -C ../docs add --renormalize -- ../src/a.rs",
		),
	] {
		let repo = TestRepo::init();
		repo.write("src/a.rs", "fn a() {}\r\n");
		repo
			.run(&["track", "docs/guide.md", "--source", "/src/a.rs", "--yes"])
			.expect_code(0);
		repo.commit_at("2026-10-01T10:00:00Z");
		// Committed with CRLF before `text=auto`, and then edited: `git add`
		// would keep the CRLF that the index holds.
		repo.write(".gitattributes", "* text=auto\n");
		std::fs::write(repo.path().join("src/a.rs"), "fn a() {}\r\nfn b() {}\r\n")
			.unwrap();
		let mut args = Vec::new();
		if let Some(repo_argument) = repo_argument {
			args.extend(["-C", repo_argument]);
		}
		args.extend(["attest", "guide.md", "--yes"]);
		let refused = repo.run_in(directory, &args);
		refused.expect_code(4);
		let command = quoted(&refused.stderr, "git ");
		assert_eq!(command, expected, "{}", refused.stderr);

		repo.run_shell_in(directory, command).expect_code(0);
		repo.run_in(directory, &args).expect_code(0);
	}
}

/// A file that git does not track, in the way of a file the plan creates,
/// is named by a `git add` that works where the command was run.
#[test]
fn the_untracked_file_hint_adds_it_where_it_was_run() {
	let repo = TestRepo::init();
	std::fs::write(repo.path().join("docs/source-sentry.lock"), "{}\n").unwrap();
	let refused =
		repo.run_in("src", &["-C", "..", "track", "docs/guide.md", "--yes"]);
	refused.expect_code(4);
	let command = quoted(&refused.stderr, "git ");
	assert_eq!(
		command, "git -C .. add -- docs/source-sentry.lock",
		"{}",
		refused.stderr
	);
	repo.run_shell_in("src", command).expect_code(0);
	assert_eq!(
		repo.git(&["ls-files", "docs/source-sentry.lock"]),
		"docs/source-sentry.lock"
	);
}

/// The advice to clear the assume-unchanged mark that keeps git from
/// comparing an edited placeholder of a symbolic link repeats `-C` and
/// names the path from the directory git starts in, so that it runs as
/// printed.
#[test]
fn the_assume_unchanged_advice_runs_where_it_was_printed() {
	let repo = TestRepo::init();
	repo.git(&["config", "core.symlinks", "false"]);
	repo.write("src/target.txt", "content\n");
	let link = repo.path().join("src/link");
	std::fs::write(&link, "x\r\ny\r\n").unwrap();
	let blob = repo.git(&["hash-object", "-w", "--no-filters", "src/link"]);
	repo.git(&[
		"update-index",
		"--add",
		"--cacheinfo",
		&format!("120000,{blob},src/link"),
	]);
	repo.write(".gitattributes", "src/link text=auto ident\n");
	repo.commit_at("2026-10-01T10:00:00Z");
	repo
		.run(&["track", "docs/guide.md", "--source", "/src/link", "--yes"])
		.expect_code(0);
	repo.commit_at("2026-10-02T10:00:00Z");
	repo.git(&["update-index", "--assume-unchanged", "src/link"]);
	std::fs::write(&link, "$Id: expanded-value $\r\nnew-target\r\n").unwrap();

	let check = ["-C", "../docs", "check", "--at", crate::support::AT];
	let refused = repo.run_in("src", &check);
	refused.expect_code(4);
	let command = quoted(&refused.stderr, "git -C");
	assert_eq!(
		command, "git -C ../docs update-index --no-assume-unchanged -- ../src/link",
		"{}",
		refused.stderr
	);
	repo.run_shell_in("src", command).expect_code(0);
	assert_ne!(repo.run_in("src", &check).code, 4);
}
