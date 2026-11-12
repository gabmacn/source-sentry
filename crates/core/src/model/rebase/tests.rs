//! Re-expression preserves source identities at every directory depth.

use super::{RebaseError, rebase_path, rebase_pattern};
use crate::claims::SourceGlob;
use crate::model::{RepoPath, resolve_source};

/// A canonical path for a test.
fn path(value: &str) -> RepoPath {
	RepoPath::new(value).unwrap()
}

/// Generated relative spellings retain a protective dot before a drive-like
/// first component, for both literal paths and glob suffixes.
#[test]
fn drive_like_first_components_keep_their_protective_dot() {
	let directory = path("audits");
	for declared in ["./C:source.rs", "./c:/source.rs"] {
		assert_eq!(
			rebase_path(declared, &directory, &directory).unwrap(),
			declared
		);
	}
	for declared in ["./C:*.rs", "./c:/*.rs"] {
		let rebased = rebase_pattern(declared, &directory, &directory).unwrap();
		assert_eq!(rebased, declared);
		assert_eq!(
			SourceGlob::compile(Some(&path("audits/v1.md")), declared)
				.unwrap()
				.rooted(),
			SourceGlob::compile(Some(&path("audits/v2.md")), &rebased)
				.unwrap()
				.rooted()
		);
	}
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
		Err(RebaseError::PatternDirectory(_))
	));
}

/// Paths retain their target across sibling, parent and root directories.
#[test]
fn paths_keep_their_target() {
	for (declared, from, to, expected) in [
		("crates/cli.rs", "", "docs", "../crates/cli.rs"),
		("../src/a.rs", "docs", "docs/deep", "../../src/a.rs"),
		("./a.rs", "src", "", "src/a.rs"),
		("a.rs", "src", "src", "a.rs"),
		(".", "src", "src", "."),
		("/src/./a.rs", "docs", "elsewhere", "/src/./a.rs"),
	] {
		assert_eq!(
			rebase_path(declared, &path(from), &path(to)).unwrap(),
			expected
		);
	}
	assert!(rebase_path("../outside", &RepoPath::ROOT, &path("docs")).is_err());
	assert!(rebase_path("", &RepoPath::ROOT, &path("docs")).is_err());
}

/// Glob syntax remains intact while the literal prefix changes base.
#[test]
fn patterns_rebase_only_the_literal_prefix() {
	for (declared, from, to, expected) in [
		("src/**/*.rs", "", "docs", "../src/**/*.rs"),
		("../src/{a,b}/*.rs", "docs", "src", "{a,b}/*.rs"),
		("*.rs", "src", "docs/deep", "../../src/*.rs"),
		("{src,lib}/**", "", "docs", "../{src,lib}/**"),
		("src/../lib/a?.rs", "", "docs", "../lib/a?.rs"),
		("src/*/fixed/../a.rs", "", "docs", "../src/*/fixed/../a.rs"),
		("src/a.rs", "", "docs", "../src/a.rs"),
		("/src/*.rs", "docs", "", "/src/*.rs"),
	] {
		assert_eq!(
			rebase_pattern(declared, &path(from), &path(to)).unwrap(),
			expected
		);
	}
}

/// Unsafe or invalid spellings carry a reason instead of changing meaning.
#[test]
fn unsafe_patterns_are_rejected() {
	for invalid in ["", "../*.rs", "src/*/../a.rs", "src/{a,b", "src\\*.rs"] {
		assert!(
			rebase_pattern(invalid, &RepoPath::ROOT, &path("docs")).is_err(),
			"{invalid}"
		);
	}
	assert!(matches!(
		rebase_pattern("*.rs", &path("src[*]"), &path("docs")),
		Err(RebaseError::PatternDirectory(_))
	));
}
