//! Queue selection, prompts, expectation refusals, and display regressions.

use serde_json::json;
use source_sentry_core::model::Outcome;
use std::fmt::Write as _;

use super::super::opening::Opening;
use super::super::{Answer, session, state::State};
use super::support::{Repo, Script, Step, local};

/// Every advertised answer works, Enter leaves a document, and `s` is absent.
#[test]
fn one_letter_answers_and_the_empty_default() {
	for (key, answer) in [
		("", Answer::Next),
		("n", Answer::Next),
		("y", Answer::Yes),
		("r", Answer::Reread),
		("a", Answer::All),
		("d", Answer::Diff),
		("e", Answer::Edit),
		("q", Answer::Quit),
	] {
		assert_eq!(Answer::parse(key), Some(answer));
	}
	assert_eq!(Answer::parse("s"), None);
	assert_eq!(Answer::parse("yes"), None);
}

/// A session opens once with the facts of its starting queue, counting named
/// documents when any were named; an empty queue has no opening.
#[test]
fn a_session_opens_with_its_facts_unless_the_queue_is_empty() {
	let repo = Repo::new(
		json!({"docs/a.md": {"sources": [{"path": "/src/a.rs"}]}, "docs/b.md": {}, "docs/fresh.md": {}}),
	);
	repo.attest("docs/fresh.md");
	let mut script = Script::answers(&repo, &[Answer::Next, Answer::Quit]);
	repo.run(&[], &mut script).unwrap();
	assert_eq!(
		script.openings,
		[Opening {
			reviewer: Some("Test Reviewer".to_owned()),
			context: "default".to_owned(),
			projects: 1,
			queued: 2,
			tracked: 3,
			outcomes: vec![(Outcome::Warn, 2)],
		}]
	);
	let mut script = Script::answers(&repo, &[Answer::Quit]);
	repo
		.run(&["docs/b.md", "docs/fresh.md"], &mut script)
		.unwrap();
	assert_eq!(
		(script.openings[0].queued, script.openings[0].tracked),
		(1, 2)
	);
	repo.attest("docs/a.md");
	repo.attest("docs/b.md");
	let mut script = Script::answers(&repo, &[]);
	repo.run(&[], &mut script).unwrap();
	assert_eq!(script.openings, []);
	assert_eq!(script.shown, []);
}

/// The display has one shape: a ruled position, the document with its
/// outcome, then each file with its counts, object names and located lines.
#[test]
fn a_document_is_displayed_with_a_rule_a_header_and_located_lines() {
	let repo =
		Repo::new(json!({"docs/a.md": {"sources": [{"path": "/src/a.rs"}]}}));
	repo.attest("docs/a.md");
	repo.write(
		"src/a.rs",
		"old a
	fn new() {}
",
	);
	let mut script = Script::answers(&repo, &[Answer::Quit]);
	repo.run(&[], &mut script).unwrap();
	let root = repo
		.workspace()
		.display_path(&source_sentry_core::model::RepoPath::new("src").unwrap());
	let shown = script.displays[0].replace(root.trim_end_matches("src"), "");
	assert_eq!(
		shown,
		[
			"",
			&format!("── 1 of 1 {}", "─".repeat(90)),
			&format!("  docs/a.md{}warn  due (review at once)", " ".repeat(63)),
			"",
			&format!("  src/a.rs{}+1 -0   ffc7496 → 30bb147", " ".repeat(65)),
			"  │ src/a.rs:1",
			"  │    1   old a",
			"  │    2 +     fn new() {}",
			"",
		]
		.join(
			"
"
		)
	);
}

/// Context severity precedes dependency and lexical ordering within a tier.
#[test]
fn queue_orders_outcomes_then_upstreams_then_paths() {
	let repo = Repo::new(
		json!({"docs/a.md": {"sources": [{"path": "z.md"}]}, "docs/b.md": {}, "docs/z.md": {"sources": [{"path": "/src/a.rs"}]}, "docs/critical.md": {"classes": ["critical"]}}),
	);
	repo.write(".source-sentry/source-sentry.yaml", "version: 1\ndefine:\n  classes:\n    critical: {}\n  contexts:\n    production:\n      fallback: warn\n      rules:\n        - classes: [critical]\n          facts: [unattested]\n          outcome: fail\n");
	let mut args = repo.args(&[]);
	args.context = "production".to_owned();
	let mut script = Script::answers(&repo, &[Answer::Next; 4]);
	session::run(&repo.workspace(), &args, &mut script).unwrap();
	assert_eq!(
		script.shown,
		[
			local("docs/critical.md"),
			local("docs/b.md"),
			local("docs/z.md"),
			local("docs/a.md")
		]
	);
	assert!(script.displays[0].contains("── 1 of 4 ─"));
}

/// Explicit selection excludes other pending documents and ignores fresh ones.
#[test]
fn named_documents_limit_the_queue_and_fresh_documents_drop_out() {
	let repo =
		Repo::new(json!({"docs/a.md": {}, "docs/b.md": {}, "docs/fresh.md": {}}));
	repo.attest("docs/fresh.md");
	let mut script = Script::answers(&repo, &[Answer::Yes]);
	repo
		.run(&["docs/b.md", "docs/fresh.md"], &mut script)
		.unwrap();
	assert_eq!(script.shown, [local("docs/b.md")]);
	assert!(script.summary().contains("1 document still needs review"));
	assert!(repo.entry("docs/fresh.md")["reviewed"].is_object());
	let mut script = Script::answers(&repo, &[]);
	assert_eq!(repo.run(&["missing.md"], &mut script).unwrap_err().code, 2);
}

/// `y` records sources immediately, while `r` also records the document reread.
#[test]
fn yes_and_reread_share_attestation_writes_and_reviewer_defaults() {
	for (answer, reread) in [(Answer::Yes, false), (Answer::Reread, true)] {
		let repo = Repo::new(
			json!({"docs/a.md": {"sources": [{"path": "/src/a.rs"}]}, "docs/b.md": {}}),
		);
		let mut args = repo.args(&[]);
		if reread {
			args.reviewer = Some("Named Reviewer".to_owned());
		}
		let mut script = Script::answers(&repo, &[answer, Answer::Quit]);
		session::run(&repo.workspace(), &args, &mut script).unwrap();
		let entry = repo.entry("docs/a.md");
		assert_eq!(entry["reviewed"].is_object(), reread);
		assert_eq!(
			entry["sources"][0]["reviewed"]["by"],
			if reread {
				"Named Reviewer"
			} else {
				"Test Reviewer"
			}
		);
		assert!(
			entry["sources"][0]["oid"]
				.as_str()
				.unwrap()
				.ends_with(&repo.git(&["hash-object", "src/a.rs"]))
		);
		assert!(repo.entry("docs/b.md")["reviewed"].is_null());
		assert!(script.summary().contains("Recorded 1 review"));
	}
}

/// Documents without sources always record a reread, for both positive keys.
#[test]
fn sourceless_yes_and_reread_both_record_a_reread() {
	for answer in [Answer::Yes, Answer::Reread] {
		let repo = Repo::new(json!({"docs/a.md": {}}));
		let mut script = Script::answers(&repo, &[answer]);
		repo.run(&[], &mut script).unwrap();
		assert!(repo.entry("docs/a.md")["reviewed"].is_object());
		assert!(script.summary().contains("0 documents still need review"));
	}
}

/// A changed source refuses the old `y`, displays a new comparison, and
/// records only after the next answer approves that comparison.
#[test]
fn changed_sources_refuse_yes_and_are_shown_again() {
	let repo =
		Repo::new(json!({"docs/a.md": {"sources": [{"path": "/src/a.rs"}]}}));
	let mut script = Script::new(
		&repo,
		[
			Step::Change("src/a.rs", "new a\n".to_owned(), Answer::Yes),
			Step::Answer(Some(Answer::Yes)),
		],
	);
	repo.run(&[], &mut script).unwrap();
	assert_eq!(script.shown, [local("docs/a.md"), local("docs/a.md")]);
	assert_ne!(
		script.comparisons[0].expectations,
		script.comparisons[1].expectations
	);
	assert!(script.messages[0].contains("was not recorded"));
	assert!(script.summary().contains("Recorded 1 review"));
}

/// Changes to pattern membership receive the same exact-expectation check.
#[test]
fn a_new_pattern_member_refuses_the_displayed_comparison() {
	let repo =
		Repo::new(json!({"docs/a.md": {"sources": [{"pattern": "/src/*.rs"}]}}));
	let mut script = Script::new(
		&repo,
		[
			Step::Change("src/c.rs", "new member\n".to_owned(), Answer::Yes),
			Step::Answer(Some(Answer::Quit)),
		],
	);
	repo.run(&[], &mut script).unwrap();
	assert_eq!(script.shown.len(), 2);
	assert!(repo.entry("docs/a.md")["sources"][0]["members"].is_null());
}

/// EOF and `q` preserve both skipped documents and unvisited documents.
#[test]
fn quitting_and_end_of_input_leave_the_rest_unchanged() {
	for answer in [None, Some(Answer::Quit)] {
		let repo = Repo::new(json!({"docs/a.md": {}, "docs/b.md": {}}));
		let before = repo.read("source-sentry.lock");
		let mut script = Script::new(&repo, [Step::Answer(answer)]);
		repo.run(&[], &mut script).unwrap();
		assert_eq!(repo.read("source-sentry.lock"), before);
		assert!(script.summary().contains("2 documents still need review"));
		assert!(
			script
				.summary()
				.contains(&format!("{} check", repo.workspace().command_prefix()))
		);
	}
}

/// Full diff viewing preserves the approved comparison and resumes its prompt.
#[test]
fn full_diff_is_not_capped_and_returns_to_the_same_prompt() {
	let repo =
		Repo::new(json!({"docs/a.md": {"sources": [{"path": "/src/a.rs"}]}}));
	repo.write(
		"src/a.rs",
		&(0..90).fold(String::new(), |mut text, index| {
			writeln!(text, "line {index}").unwrap();
			text
		}),
	);
	let mut script = Script::answers(&repo, &[Answer::Diff, Answer::Next]);
	repo.run(&[], &mut script).unwrap();
	assert_eq!(script.shown.len(), 1);
	assert!(script.displays[0].contains("+90 -0   unattested → "));
	assert!(script.displays[0].contains("50 more diff lines · d opens"));
	assert!(script.diffs[0].contains("  90 + line 89"));
	assert!(!script.displays[0].contains("+ line 89"));
	let source = repo.workspace().display_path(
		&source_sentry_core::model::RepoPath::new("src/a.rs").unwrap(),
	);
	assert!(script.displays[0].contains(&format!("{source}:1")));
}

/// Configuration errors and dependency cycles refuse before a prompt or write.
#[test]
fn configuration_errors_refuse_before_the_first_prompt() {
	let repo = Repo::new(
		json!({"docs/a.md": {"sources": [{"path": "b.md"}]}, "docs/b.md": {"sources": [{"path": "a.md"}]}}),
	);
	let mut script = Script::answers(&repo, &[Answer::Yes]);
	assert_eq!(repo.run(&[], &mut script).unwrap_err().code, 3);
	assert_eq!(script.shown, []);
	let other = Repo::new(json!({"docs/a.md": {}}));
	assert_eq!(
		State::load(&other.workspace(), "missing")
			.unwrap()
			.fallbacks
			.len(),
		1
	);
}

/// A sealed record is never queued even when explicitly selected.
#[test]
fn sealed_records_are_never_queued() {
	let repo = Repo::new(json!({"docs/a.md": {}}));
	let oid = repo.git(&["hash-object", "docs/a.md"]);
	repo.write("source-sentry.lock", &json!({"schema": 1, "retired": {}, "documents": {"docs/a.md": {"seal": {"oid": format!("sha1:{oid}"), "at": "2026-10-02T10:00:00Z", "by": "Earlier Reviewer"}}}}).to_string());
	let mut script = Script::answers(&repo, &[Answer::Yes]);
	repo.run(&["docs/a.md"], &mut script).unwrap();
	assert_eq!(script.shown, []);
}

/// Committed drift whose deadline has passed is queued and can be reviewed
/// with either positive answer.
#[test]
fn overdue_documents_are_queued_and_reviewed() {
	for answer in [Answer::Yes, Answer::Reread] {
		let repo =
			Repo::new(json!({"docs/a.md": {"sources": [{"path": "/src/a.rs"}]}}));
		repo.attest("docs/a.md");
		repo.commit();
		repo.write("src/a.rs", "committed change\n");
		repo.commit();
		let mut script = Script::answers(&repo, &[answer]);
		repo.run(&[], &mut script).unwrap();
		assert!(script.displays[0].contains("overdue"));
		assert_eq!(script.shown, [local("docs/a.md")]);
		assert!(script.summary().contains("0 documents still need review"));
	}
}
