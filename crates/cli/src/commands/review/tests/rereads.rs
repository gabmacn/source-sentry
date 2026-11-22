//! Documents whose reread is due: what `y`, `r` and `a` record for them.

use serde_json::{Value, json};

use super::super::Answer;
use super::support::{Repo, Script, Step, local};

/// When the earlier reviewer reread every document of the fixtures.
const EARLIER: &str = "2026-10-02T10:00:00Z";

/// A declaration with `sources`, due to be reread on the first of every
/// month. Reread on 2 October, its reread has been overdue since 2
/// November.
fn rereads(sources: &[&str]) -> Value {
	let sources: Vec<Value> =
		sources.iter().map(|path| json!({"path": path})).collect();
	json!({
		"schedule": {"reread": {"dtstart": "2026-01-01", "rrule": "FREQ=MONTHLY"}},
		"sources": sources,
	})
}

/// A fixture of `entries`, each reviewed and reread by the earlier
/// reviewer, and committed.
fn reviewed(entries: Value) -> Repo {
	let documents: Vec<String> =
		entries.as_object().unwrap().keys().cloned().collect();
	let repo = Repo::new(entries);
	for document in &documents {
		repo.attest(document);
	}
	repo.commit();
	repo
}

/// When `document` was last reread, as its declaration stores it.
fn reread_at(repo: &Repo, document: &str) -> Value {
	repo.entry(document)["reviewed"]["at"].clone()
}

/// A document whose reread is all it needs is presented, and `y` records
/// the reread, since nothing else is left to confirm; with or without
/// sources.
#[test]
fn yes_records_a_reread_when_no_source_changed() {
	let repo = reviewed(json!({
		"docs/a.md": rereads(&["/src/a.rs"]),
		"docs/b.md": rereads(&[]),
	}));
	let mut script = Script::answers(&repo, &[Answer::Yes, Answer::Yes]);
	repo.run(&[], &mut script).unwrap();

	assert_eq!(script.shown, [local("docs/a.md"), local("docs/b.md")]);
	assert!(
		script.displays[0].contains("Its reread is due: `y` and `r` both"),
		"{}",
		script.displays[0]
	);
	for document in ["docs/a.md", "docs/b.md"] {
		assert_ne!(reread_at(&repo, document), EARLIER, "{document}");
	}
	assert!(script.messages.iter().any(|message| {
		message.starts_with("Recorded ")
			&& message.contains("a.md")
			&& message.ends_with(" (reread).")
	}));
	assert!(script.summary().contains("0 documents still need review"));
}

/// With a changed source, `y` records the sources alone and the reread
/// stays due, while `r` records both.
#[test]
fn with_a_changed_source_yes_leaves_the_reread_due() {
	for (answer, reread) in [(Answer::Yes, false), (Answer::Reread, true)] {
		let repo = reviewed(json!({"docs/a.md": rereads(&["/src/a.rs"])}));
		repo.write("src/a.rs", "new a\n");
		repo.commit();
		let mut script = Script::answers(&repo, &[answer]);
		repo.run(&[], &mut script).unwrap();

		assert!(
			script.displays[0].contains("Its reread is due too"),
			"{}",
			script.displays[0]
		);
		let entry = repo.entry("docs/a.md");
		assert!(
			entry["sources"][0]["oid"]
				.as_str()
				.unwrap()
				.ends_with(&repo.git(&["hash-object", "src/a.rs"]))
		);
		assert_eq!(reread_at(&repo, "docs/a.md") != EARLIER, reread);
		assert_eq!(
			script.summary().contains("reread-overdue"),
			!reread,
			"{}",
			script.summary()
		);
	}
}

/// The fixture of the bulk answer: `docs/a.md`, whose reread is all it
/// needs, `docs/b.md`, without sources and never reviewed, and `docs/c.md`,
/// whose source changed.
fn bulk() -> Repo {
	let repo = reviewed(json!({
		"docs/a.md": rereads(&["/src/a.rs"]),
		"docs/c.md": {"sources": [{"path": "/src/b.rs"}]},
	}));
	let lockfile = repo.read("source-sentry.lock");
	let mut lockfile: Value = serde_json::from_str(&lockfile).unwrap();
	lockfile["documents"]["docs/b.md"] = json!({});
	repo.write("docs/b.md", "# B\n");
	repo.write(
		"source-sentry.lock",
		&serde_json::to_string(&lockfile).unwrap(),
	);
	repo.write("src/b.rs", "new b\n");
	repo.commit();
	repo
}

/// `a` names the documents it can only record by a reread, here one whose
/// reread is all it needs and one without sources, and leaves them for
/// answers of their own when the reviewer does not include them; the rest
/// is recorded.
#[test]
fn all_leaves_the_rereads_unless_they_are_included() {
	let repo = bulk();
	let mut script = Script::new(
		&repo,
		[Step::Answer(Some(Answer::All)), Step::Confirm(Some(true))],
	);
	repo.run(&[], &mut script).unwrap();

	assert_eq!(script.confirmations, [(3, 2)]);
	assert_eq!(script.rereads, [2]);
	// Each is named, with why, and marked when it was not seen.
	assert_eq!(
		script.listings,
		[concat!(
			"  docs/a.md  its reread is due and no source changed\n",
			"  docs/b.md  it has no sources, not seen yet\n",
		)]
	);
	assert_eq!(reread_at(&repo, "docs/a.md"), EARLIER);
	assert!(repo.entry("docs/b.md")["reviewed"].is_null());
	assert_ne!(
		repo.entry("docs/c.md")["sources"][0]["reviewed"]["at"],
		EARLIER,
		"the stale document was recorded"
	);
	assert!(script.summary().contains("docs/a.md: reread-overdue"));
}

/// Included, the documents that can only be recorded by a reread are each
/// recorded as reread, with the rest of the batch.
#[test]
fn all_records_the_included_rereads_as_reread() {
	let repo = bulk();
	let mut script = Script::new(
		&repo,
		[Step::Answer(Some(Answer::All)), Step::ConfirmRereads],
	);
	repo.run(&[], &mut script).unwrap();

	assert_ne!(reread_at(&repo, "docs/a.md"), EARLIER);
	assert!(repo.entry("docs/b.md")["reviewed"].is_object());
	assert_eq!(reread_at(&repo, "docs/c.md"), EARLIER);
	assert!(script.summary().contains("0 documents still need review"));
}

/// When every remaining document can only be recorded by a reread, `a`
/// asks about them alone: declining records nothing, and including them
/// records each as reread.
#[test]
fn all_asks_about_the_rereads_when_only_they_remain() {
	for include in [false, true] {
		let repo = reviewed(json!({"docs/a.md": rereads(&[])}));
		let answer = if include {
			Step::ConfirmRereads
		} else {
			Step::Confirm(Some(false))
		};
		let mut script =
			Script::new(&repo, [Step::Answer(Some(Answer::All)), answer]);
		repo.run(&[], &mut script).unwrap();

		assert_eq!(script.rereads, [1]);
		// Shown before `a`, so it is not marked as unseen.
		assert_eq!(script.listings, ["  docs/a.md  it has no sources\n"]);
		assert_eq!(reread_at(&repo, "docs/a.md") != EARLIER, include);
	}
}
