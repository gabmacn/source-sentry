//! Re-expression preserves source identities at every directory depth.

use super::{RebaseError, rebase_path, rebase_pattern};
use crate::claims::SourceGlob;
use crate::model::{RepoPath, resolve_source};

/// A canonical path for a test.
fn path(value: &str) -> RepoPath {
	RepoPath::new(value).unwrap()
}

/// Paths and patterns retain their repository identities after rebasing.
#[test]
fn directories_and_rooted_spellings() {
	for (from, to, declared, expected) in [
		("docs", "docs", "../src/a.rs", "../src/a.rs"),
		("docs", "docs/deep", "../src/a.rs", "../../src/a.rs"),
		("docs/deep", "docs", "../../src/a.rs", "../src/a.rs"),
		("docs/old", "docs/new", "a.rs", "../old/a.rs"),
		("docs", "", "../src/a.rs", "src/a.rs"),
		("", "docs", "src/a.rs", "../src/a.rs"),
		("docs", "other", "/src/./a.rs", "/src/./a.rs"),
	] {
		let rebased = rebase_path(declared, &path(from), &path(to)).unwrap();
		assert_eq!(rebased, expected);
		assert_eq!(
			resolve_source(
				&path(format!("{from}/page.md").trim_start_matches('/')),
				declared
			),
			resolve_source(
				&path(format!("{to}/page.md").trim_start_matches('/')),
				&rebased
			)
		);
	}
	for (from, to, declared, expected) in [
		("docs", "docs", "../src/**/*.rs", "../src/**/*.rs"),
		(
			"docs",
			"docs/deep",
			"../src/{a,b}/*.rs",
			"../../src/{a,b}/*.rs",
		),
		("docs/deep", "docs", "../../src/[ab]?.rs", "../src/[ab]?.rs"),
		("docs/old", "docs/new", "*.rs", "../old/*.rs"),
		("docs/old", "docs", "../*.rs", "*.rs"),
		("docs", "other", "/src/**/*.rs", "/src/**/*.rs"),
	] {
		let rebased = rebase_pattern(declared, &path(from), &path(to)).unwrap();
		assert_eq!(rebased, expected);
		let original =
			SourceGlob::compile(Some(&path(&format!("{from}/page.md"))), declared)
				.unwrap();
		let moved =
			SourceGlob::compile(Some(&path(&format!("{to}/page.md"))), &rebased)
				.unwrap();
		assert_eq!(original.rooted(), moved.rooted());
	}
}

/// Invalid spellings are refused instead of losing sources.
#[test]
fn refuses_unresolvable_sources() {
	assert!(
		rebase_path("../../src/a.rs", &path("docs"), &path("other")).is_err()
	);
	assert!(rebase_path("", &path("docs"), &path("other")).is_err());
	assert_eq!(
		rebase_pattern("*/../a.rs", &path("docs"), &path("other")),
		Err(RebaseError::ParentAfterWildcard)
	);
	assert!(matches!(
		rebase_pattern("*.rs", &path("docs/[old]"), &path("other")),
		Err(RebaseError::PatternPrefix(_))
	));
}
