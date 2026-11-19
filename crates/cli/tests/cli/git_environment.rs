//! Git's environment variables: the repository, working tree, index and
//! object directories they name are the ones Source Sentry uses, as git
//! itself would with the same directory and environment.

use std::ffi::OsStr;

use crate::support::{AT, TestRepo};

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
