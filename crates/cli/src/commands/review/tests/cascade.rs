//! Queue rebuilding, skipped and reviewed checkpoints, and bulk review bounds.

use serde_json::json;

use super::super::Answer;
use super::support::{Repo, Script, Step, local};

/// Reviewing an upstream drops dependents whose only pending fact clears.
#[test]
fn clearing_upstream_staleness_drops_the_dependent_without_an_answer() {
	let repo = Repo::new(
		json!({"docs/index.md": {"sources": [{"path": "z.md"}]}, "docs/z.md": {"sources": [{"path": "/src/a.rs"}]}}),
	);
	repo.attest("docs/z.md");
	repo.attest("docs/index.md");
	repo.commit();
	repo.write("src/a.rs", "changed\n");
	let mut script = Script::answers(&repo, &[Answer::Yes]);
	repo.run(&[], &mut script).unwrap();
	assert_eq!(script.shown, [local("docs/z.md")]);
	assert!(script.summary().contains("0 documents still need review"));
}

/// An unchanged skipped document stays skipped through unrelated writes.
#[test]
fn next_does_not_repeat_after_an_unrelated_write() {
	let repo = Repo::new(
		json!({"docs/a.md": {"sources": [{"path": "/src/a.rs"}]}, "docs/b.md": {}}),
	);
	let mut script = Script::answers(&repo, &[Answer::Next, Answer::Yes]);
	repo.run(&[], &mut script).unwrap();
	assert_eq!(script.shown, [local("docs/a.md"), local("docs/b.md")]);
	assert!(script.summary().contains("1 document still needs review"));
}

/// `n` skips the state at its answer, including changes made while the
/// document's prompt was open; only a subsequent change can queue it again.
#[test]
fn next_does_not_retry_a_change_that_precedes_the_answer() {
	let repo =
		Repo::new(json!({"docs/a.md": {"sources": [{"path": "/src/a.rs"}]}}));
	let mut script = Script::new(
		&repo,
		[Step::Change(
			"src/a.rs",
			"changed before n\n".to_owned(),
			Answer::Next,
		)],
	);
	repo.run(&[], &mut script).unwrap();
	assert_eq!(script.shown, [local("docs/a.md")]);
	assert!(script.summary().contains("1 document still needs review"));
}

/// A skipped document returns only after one of its sources changes.
#[test]
fn an_edit_requeues_a_skipped_document_with_changed_sources() {
	let repo = Repo::new(
		json!({"docs/a.md": {"sources": [{"path": "/src/a.rs"}]}, "docs/b.md": {}}),
	);
	let mut script = Script::new(
		&repo,
		[
			Step::Answer(Some(Answer::Next)),
			Step::Answer(Some(Answer::Edit)),
			Step::Edit(vec![("src/a.rs", "edited source\n".to_owned())]),
			Step::Answer(Some(Answer::Yes)),
			Step::Answer(Some(Answer::Yes)),
		],
	);
	repo.run(&[], &mut script).unwrap();
	assert_eq!(
		script.shown,
		[
			local("docs/a.md"),
			local("docs/b.md"),
			local("docs/b.md"),
			local("docs/a.md")
		]
	);
	assert!(
		script
			.displays
			.last()
			.unwrap()
			.contains("Sources changed during this session")
	);
	assert!(script.summary().contains("0 documents still need review"));
}

/// A previously recorded review returns after a later edit changes a source.
#[test]
fn a_reviewed_document_returns_after_a_later_change() {
	let repo = Repo::new(
		json!({"docs/a.md": {"sources": [{"path": "/src/a.rs"}]}, "docs/b.md": {}}),
	);
	let mut script = Script::new(
		&repo,
		[
			Step::Answer(Some(Answer::Yes)),
			Step::Answer(Some(Answer::Edit)),
			Step::Edit(vec![("src/a.rs", "edited after review\n".to_owned())]),
			Step::Answer(Some(Answer::Yes)),
			Step::Answer(Some(Answer::Reread)),
		],
	);
	repo.run(&[], &mut script).unwrap();
	assert_eq!(
		script.shown,
		[
			local("docs/a.md"),
			local("docs/b.md"),
			local("docs/b.md"),
			local("docs/a.md")
		]
	);
	assert!(script.summary().contains("Recorded 3 reviews"));
	assert!(
		script
			.displays
			.last()
			.unwrap()
			.contains("Sources changed during this session")
	);
}

/// Re-evaluation adds documents that were fresh before an editor visit.
#[test]
fn newly_stale_documents_join_the_remaining_queue() {
	let repo = Repo::new(
		json!({"docs/a.md": {}, "docs/b.md": {"sources": [{"path": "/src/b.rs"}]}}),
	);
	repo.attest("docs/b.md");
	repo.commit();
	let mut script = Script::new(
		&repo,
		[
			Step::Answer(Some(Answer::Edit)),
			Step::Edit(vec![
				("docs/a.md", "# Revised document\n".to_owned()),
				("src/b.rs", "edited source\n".to_owned()),
			]),
			Step::Answer(Some(Answer::Yes)),
			Step::Answer(Some(Answer::Yes)),
		],
	);
	repo.run(&[], &mut script).unwrap();
	assert_eq!(
		script.shown,
		[local("docs/a.md"), local("docs/a.md"), local("docs/b.md")]
	);
	assert!(script.displays[2].contains("Sources changed during this session"));
	assert!(script.summary().contains("0 documents still need review"));
}

/// Editing a document changes the diff of dependents that use its contents.
#[test]
fn editing_a_source_document_is_seen_by_its_dependents() {
	let repo = Repo::new(
		json!({"docs/a.md": {}, "docs/index.md": {"sources": [{"path": "a.md"}]}}),
	);
	repo.attest("docs/index.md");
	repo.commit();
	let mut script = Script::new(
		&repo,
		[
			Step::Answer(Some(Answer::Edit)),
			Step::Edit(vec![("docs/a.md", "# Edited document\n".to_owned())]),
			Step::Answer(Some(Answer::Yes)),
			Step::Answer(Some(Answer::Yes)),
		],
	);
	repo.run(&[], &mut script).unwrap();
	assert_eq!(
		script.shown,
		[
			local("docs/a.md"),
			local("docs/a.md"),
			local("docs/index.md")
		]
	);
	assert!(script.displays[2].contains("+1 -1 lines"));
	assert!(script.displays[2].contains("+# Edited document"));
	assert!(script.summary().contains("0 documents still need review"));
}

/// Named sessions report newly stale obligations beyond their selection.
#[test]
fn newly_stale_documents_outside_the_selection_are_reported_at_the_end() {
	let repo = Repo::new(
		json!({"docs/a.md": {}, "docs/b.md": {"sources": [{"path": "/src/b.rs"}]}}),
	);
	repo.attest("docs/b.md");
	repo.commit();
	let mut script = Script::new(
		&repo,
		[
			Step::Answer(Some(Answer::Edit)),
			Step::Edit(vec![("src/b.rs", "changed outside selection\n".to_owned())]),
			Step::Answer(Some(Answer::Yes)),
		],
	);
	repo.run(&["docs/a.md"], &mut script).unwrap();
	assert!(!script.shown.contains(&local("docs/b.md")));
	assert!(script.summary().contains(
		"started needing review during this session; outside the selected documents"
	));
	assert!(script.summary().contains("1 document still needs review"));
}

/// The closing summary reflects the final working tree after a quit.
#[test]
fn quit_summaries_evaluate_changes_made_while_the_last_prompt_was_open() {
	let repo = Repo::new(
		json!({"docs/a.md": {}, "docs/b.md": {"sources": [{"path": "/src/b.rs"}]}}),
	);
	repo.attest("docs/b.md");
	repo.commit();
	let mut script = Script::new(
		&repo,
		[Step::Change(
			"src/b.rs",
			"changed before quit\n".to_owned(),
			Answer::Quit,
		)],
	);
	repo.run(&[], &mut script).unwrap();
	assert!(script.summary().contains("2 documents still need review"));
	assert!(
		script
			.summary()
			.contains("started needing review during this session")
	);
}

/// Bulk confirmation counts unseen documents and records the confirmed batch.
#[test]
fn all_confirms_the_unseen_count_and_records_each_remaining_document() {
	let repo =
		Repo::new(json!({"docs/a.md": {}, "docs/b.md": {}, "docs/c.md": {}}));
	let mut script = Script::new(
		&repo,
		[Step::Answer(Some(Answer::All)), Step::Confirm(Some(true))],
	);
	repo.run(&[], &mut script).unwrap();
	assert_eq!(script.confirmations, [(3, 2)]);
	assert_eq!(script.shown, [local("docs/a.md")]);
	assert!(script.summary().contains("Recorded 3 reviews"));
	assert!(script.summary().contains("0 documents still need review"));
}

/// A previously shown document whose sources changed has an unseen comparison
/// until its new version is displayed.
#[test]
fn bulk_confirmation_counts_changed_versions_as_unseen() {
	let repo = Repo::new(
		json!({"docs/a.md": {"sources": [{"path": "/src/a.rs"}]}, "docs/b.md": {}}),
	);
	let mut script = Script::new(
		&repo,
		[
			Step::Answer(Some(Answer::Next)),
			Step::Answer(Some(Answer::Edit)),
			Step::Edit(vec![("src/a.rs", "new version\n".to_owned())]),
			Step::Answer(Some(Answer::All)),
			Step::Confirm(Some(true)),
		],
	);
	repo.run(&[], &mut script).unwrap();
	assert_eq!(script.confirmations, [(2, 1)]);
	assert!(script.summary().contains("0 documents still need review"));
}

/// Declining a bulk confirmation returns to the document without a write.
#[test]
fn declining_all_or_ending_input_during_confirmation_never_writes() {
	for confirmation in [Some(false), None] {
		let repo = Repo::new(json!({"docs/a.md": {}, "docs/b.md": {}}));
		let before = repo.read("source-sentry.lock");
		let mut script = Script::new(
			&repo,
			[
				Step::Answer(Some(Answer::All)),
				Step::Confirm(confirmation),
				Step::Answer(Some(Answer::Quit)),
			],
		);
		repo.run(&[], &mut script).unwrap();
		assert_eq!(repo.read("source-sentry.lock"), before);
	}
}

/// Each bulk write rebuilds the queue, dropping dependents that become fresh.
#[test]
fn bulk_review_drops_documents_that_no_longer_need_review() {
	let repo = Repo::new(
		json!({"docs/a.md": {"sources": [{"path": "/src/a.rs"}]}, "docs/index.md": {"sources": [{"path": "a.md"}]}}),
	);
	repo.attest("docs/a.md");
	repo.attest("docs/index.md");
	repo.commit();
	let index = repo.entry("docs/index.md");
	repo.write("src/a.rs", "changed\n");
	let mut script = Script::new(
		&repo,
		[Step::Answer(Some(Answer::All)), Step::Confirm(Some(true))],
	);
	repo.run(&[], &mut script).unwrap();
	assert_eq!(script.confirmations, [(2, 1)]);
	assert_eq!(repo.entry("docs/index.md"), index);
	assert!(script.summary().contains("Recorded 1 review"));
}

/// A source edited after the first bulk write refuses the later confirmed
/// review rather than silently approving the newer source version.
#[test]
fn bulk_expectations_remain_frozen_across_the_batch() {
	let repo = Repo::new(
		json!({"docs/a.md": {}, "docs/b.md": {"sources": [{"path": "/src/b.rs"}]}}),
	);
	let mut script = Script::new(
		&repo,
		[
			Step::Answer(Some(Answer::All)),
			Step::Confirm(Some(true)),
			Step::ChangeAfterReview(
				"src/b.rs",
				"edited during bulk review\n".to_owned(),
			),
			Step::Answer(Some(Answer::Quit)),
		],
	);
	repo.run(&[], &mut script).unwrap();
	assert_eq!(script.confirmations, [(2, 1)]);
	assert_eq!(script.shown, [local("docs/a.md"), local("docs/b.md")]);
	assert!(script.displays[1].contains("+edited during bulk review"));
	assert!(
		script
			.messages
			.iter()
			.any(|message| message.contains("was not recorded"))
	);
	assert!(repo.entry("docs/b.md")["sources"][0]["oid"].is_null());
	assert!(script.summary().contains("Recorded 1 review"));
	assert!(script.summary().contains("1 document still needs review"));
}

/// A source edit during bulk review gives a newly stale document its own
/// prompt, without extending the confirmed batch.
#[test]
fn a_newly_stale_document_outside_the_batch_gets_an_individual_prompt() {
	let repo = Repo::new(
		json!({"docs/a.md": {}, "docs/b.md": {"sources": [{"path": "/src/b.rs"}]}}),
	);
	repo.attest("docs/b.md");
	repo.commit();
	let before = repo.entry("docs/b.md");
	let mut script = Script::new(
		&repo,
		[
			Step::Answer(Some(Answer::All)),
			Step::Confirm(Some(true)),
			Step::ChangeAfterReview(
				"src/b.rs",
				"edited outside the batch\n".to_owned(),
			),
			Step::Answer(Some(Answer::Quit)),
		],
	);
	repo.run(&[], &mut script).unwrap();
	assert_eq!(script.confirmations, [(1, 0)]);
	assert_eq!(script.shown, [local("docs/a.md"), local("docs/b.md")]);
	assert!(script.displays[1].contains("Sources changed during this session"));
	assert!(script.displays[1].contains("+edited outside the batch"));
	assert_eq!(repo.entry("docs/b.md"), before);
	assert!(script.summary().contains("Recorded 1 review"));
	assert!(script.summary().contains("1 document still needs review"));
}
