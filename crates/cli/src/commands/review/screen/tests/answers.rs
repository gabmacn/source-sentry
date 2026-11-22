//! What an answer records: exactly what the reviewer was shown, for the
//! document on display.

use std::rc::Rc;

use serde_json::json;
use source_sentry_core::evaluate::SourceState;

use super::super::super::session::CHANGED;
use super::super::super::tests::support::Repo;
use super::super::app::{
	App, Area, Cursor, Field, Focus, Kept, Key, Overlay, Target,
};
use super::{drawn, press, show, shows};

/// A source changed on disk after its changes were shown refuses a review
/// of the picked sources, as it refuses a review of the whole document:
/// nothing is written, and the new changes are shown before another answer
/// records them.
#[test]
fn a_picked_source_changed_after_it_was_shown_refuses_the_answer() {
	let repo =
		Repo::new(json!({"docs/a.md": {"sources": [{"path": "/src/a.rs"}]}}));
	let (workspace, args) = (repo.workspace(), repo.args(&[]));
	let mut app = App::new(&workspace, &args).unwrap();
	assert!(shows(&drawn(&app, 120, 30), "+ old a"));
	press(&mut app, &[Key::Right, Key::Char(' ')]);
	assert_eq!(app.picked_sources.len(), 1);
	repo.write("src/a.rs", "changed while shown\n");
	assert!(!shows(&drawn(&app, 120, 30), "changed while shown"));

	press(&mut app, &[Key::Char('y'), Key::Enter]);
	assert!(repo.entry("docs/a.md")["sources"][0]["reviewed"].is_null());
	assert_eq!(app.session.recorded, Vec::new());
	assert_eq!(app.notice.as_deref(), Some(CHANGED));
	assert!(shows(&drawn(&app, 120, 30), "+ changed while shown"));

	press(&mut app, &[Key::Right, Key::Char(' ')]);
	press(&mut app, &[Key::Char('y'), Key::Enter]);
	let entry = repo.entry("docs/a.md");
	assert_eq!(entry["sources"][0]["reviewed"]["by"], "Test Reviewer");
	assert_eq!(
		entry["sources"][0]["oid"],
		format!("sha1:{}", repo.git(&["hash-object", "src/a.rs"]))
	);
}

/// A review of a picked pattern binds every file it was shown, including
/// one that left the pattern, and leaves the other sources as they were.
#[test]
fn a_picked_pattern_is_recorded_as_it_was_shown() {
	let repo = Repo::new(json!({"docs/a.md": {"sources": [
		{"path": "/notes.txt"},
		{"pattern": "/src/*.rs"},
	]}}));
	repo.write("notes.txt", "notes\n");
	repo.git(&["add", "--all"]);
	repo.git(&["commit", "--quiet", "-m", "Notes"]);
	repo.attest("docs/a.md");
	repo.git(&["add", "--all"]);
	repo.git(&["commit", "--quiet", "-m", "Reviewed"]);
	repo.write("notes.txt", "new notes\n");
	repo.write("src/a.rs", "new a\n");
	repo.git(&["rm", "--quiet", "src/b.rs"]);
	let (workspace, args) = (repo.workspace(), repo.args(&[]));
	let mut app = App::new(&workspace, &args).unwrap();
	assert_eq!(app.shown.as_ref().unwrap().owners, [0, 1, 1]);
	press(&mut app, &[Key::Right, Key::Down, Key::Char(' ')]);
	assert_eq!((app.focus, app.picked_sources.len()), (Focus::Sources, 1));

	press(&mut app, &[Key::Char('y'), Key::Enter]);
	let entry = repo.entry("docs/a.md");
	assert_eq!(entry["sources"][1]["reviewed"]["by"], "Test Reviewer");
	let members = entry["sources"][1]["members"].as_array().unwrap();
	assert_eq!(members.len(), 1, "{entry}");
	assert_eq!(members[0]["path"], "/src/a.rs");
	assert_eq!(entry["sources"][0]["reviewed"]["by"], "Earlier Reviewer");
	assert!(
		app
			.notice
			.as_ref()
			.unwrap()
			.contains("1 picked source alone")
	);
	assert_eq!(app.open.len(), 1, "its other source still waits");
}

/// A held answer belongs to the document it was given for: dragging onto
/// another document's row withdraws it, as moving there with a key does,
/// so Enter then records nothing.
#[test]
fn a_drag_onto_another_document_withdraws_a_held_answer() {
	let repo = Repo::new(json!({"docs/a.md": {}, "docs/b.md": {}}));
	let (workspace, args) = (repo.workspace(), repo.args(&[]));
	let mut app = App::new(&workspace, &args).unwrap();
	app.click(Target::Row(Cursor::Open(0))).unwrap();
	// Without sources, a yes records a reread, which its question names.
	press(&mut app, &[Key::Char('y')]);
	assert_eq!(app.armed, Some(true));

	app.drag(Target::Row(Cursor::Open(1)));
	assert!(app.shown.as_ref().unwrap().name.ends_with("b.md"));
	assert_eq!((app.armed, app.picked.len()), (None, 2));
	press(&mut app, &[Key::Enter]);
	assert_eq!(app.session.recorded, Vec::new());
	assert!(repo.entry("docs/a.md")["reviewed"].is_null());
	assert!(repo.entry("docs/b.md")["reviewed"].is_null());
}

/// Leaving the header returns the cursor to the document on display. When
/// that document is in the other documents and their list is closed, the
/// cursor goes to the first in the queue, and the pane and the next answer
/// go with it.
#[test]
fn leaving_the_header_keeps_the_cursor_and_the_pane_on_one_document() {
	let repo = Repo::new(json!({"docs/a.md": {}, "docs/b.md": {}}));
	let (workspace, args) = (repo.workspace(), repo.args(&["docs/a.md"]));
	let mut app = App::new(&workspace, &args).unwrap();
	press(
		&mut app,
		&[Key::Char('o'), Key::Down, Key::Down, Key::Char('o')],
	);
	assert!(!app.expanded);
	assert!(app.shown.as_ref().unwrap().name.ends_with("b.md"));
	app.click(Target::Field(Field::Projects)).unwrap();
	press(&mut app, &[Key::Escape, Key::Down]);
	assert_eq!(app.cursor, Cursor::Open(0));
	assert!(app.shown.as_ref().unwrap().name.ends_with("a.md"));

	press(&mut app, &[Key::Char('y'), Key::Enter]);
	assert!(repo.entry("docs/a.md")["reviewed"].is_object());
	assert!(repo.entry("docs/b.md")["reviewed"].is_null());
}

/// A document whose text changed on disk is read again after the next
/// evaluation, so that the pane shows its current text before a reread is
/// recorded for it.
#[test]
fn a_document_edited_outside_the_screen_is_shown_as_it_is_now() {
	let repo = Repo::new(json!({"docs/a.md": {}, "docs/b.md": {}}));
	repo.write("docs/b.md", "Old text\n");
	let (workspace, args) = (repo.workspace(), repo.args(&[]));
	let mut app = App::new(&workspace, &args).unwrap();
	while app.prefetch() {}
	repo.write("docs/b.md", "Current text\n");
	press(&mut app, &[Key::Char('y'), Key::Enter, Key::Char('v')]);
	assert!(app.shown.as_ref().unwrap().name.ends_with("b.md"));
	let rows = drawn(&app, 120, 30);
	assert!(shows(&rows, "Current text"), "{rows:#?}");
	assert!(!shows(&rows, "Old text"));
}

/// A document whose stored review of a source was removed is compared
/// again after the next evaluation, although its sources and facts are
/// unchanged: the pane shows that source as never reviewed, not as
/// changed since the review that is gone.
#[test]
fn a_document_whose_review_changed_outside_the_screen_is_compared_again() {
	let repo = Repo::new(json!({
		"docs/a.md": {},
		"docs/b.md": {"sources": [{"path": "/src/a.rs"}, {"path": "/src/b.rs"}]},
	}));
	repo.attest("docs/b.md");
	repo.git(&["add", "--all"]);
	repo.git(&["commit", "--quiet", "-m", "Reviewed"]);
	repo.write("src/a.rs", "new a\n");
	repo.write("src/b.rs", "new b\n");
	repo.git(&["add", "--all"]);
	repo.git(&["commit", "--quiet", "-m", "Changed"]);
	let (workspace, args) = (repo.workspace(), repo.args(&[]));
	let mut app = App::new(&workspace, &args).unwrap();
	while app.prefetch() {}
	let b = app
		.open
		.iter()
		.position(|id| id.document.to_string() == "docs/b.md")
		.unwrap();
	let id = app.open[b].clone();
	let before = Rc::clone(&app.cache[&id]);
	assert_eq!(before.reports[0].3, SourceState::Drifted);

	let mut lock: serde_json::Value =
		serde_json::from_str(&repo.read("source-sentry.lock")).unwrap();
	let source = lock["documents"]["docs/b.md"]["sources"][0]
		.as_object_mut()
		.unwrap();
	source.remove("oid");
	source.remove("reviewed");
	repo.write("source-sentry.lock", &lock.to_string());
	let a = app
		.open
		.iter()
		.position(|id| id.document.to_string() == "docs/a.md")
		.unwrap();
	app.click(Target::Row(Cursor::Open(a))).unwrap();
	press(&mut app, &[Key::Char('y'), Key::Enter]);
	assert!(
		app
			.cache
			.get(&id)
			.is_none_or(|now| !Rc::ptr_eq(&before, now))
	);

	let b = app.open.iter().position(|open| open == &id).unwrap();
	app.click(Target::Row(Cursor::Open(b))).unwrap();
	let rows = app.every_source();
	assert_eq!(rows[0].kept, Some(Kept::New));
	assert_eq!(rows[1].kept, Some(Kept::Changed));
}

/// A picked document that needs review is recorded when its confirmation
/// lists it, even when it is among the other documents of a session started
/// for named documents.
#[test]
fn a_confirmed_document_outside_the_named_ones_is_recorded() {
	let repo = Repo::new(
		json!({"docs/a.md": {}, "docs/b.md": {"sources": [{"path": "/src/a.rs"}]}}),
	);
	let (workspace, args) = (repo.workspace(), repo.args(&["docs/a.md"]));
	let mut app = App::new(&workspace, &args).unwrap();
	press(
		&mut app,
		&[
			Key::Char('o'),
			Key::Down,
			Key::Down,
			Key::Char(' '),
			Key::Char('y'),
		],
	);
	let Some(Overlay::Picked(comparisons, _, false)) = &app.overlay else {
		panic!("expected the confirmation");
	};
	assert_eq!(comparisons.len(), 1);
	assert_eq!(comparisons[0].0.document.to_string(), "docs/b.md");

	press(&mut app, &[Key::Char('y')]);
	assert!(repo.entry("docs/b.md")["sources"][0]["reviewed"].is_object());
	assert!(repo.entry("docs/a.md")["reviewed"].is_null());
	assert_eq!(app.session.recorded.len(), 1);
	assert_eq!(app.notice.as_deref(), Some("Recorded 1 review."));
}

/// A write that fails partway through a batch ends the session without
/// hiding the reviews already written: the failure says how many were
/// recorded and which document was not, and the closing lists them. A
/// lockfile made read-only refuses its replacement on Windows.
#[test]
#[cfg(windows)]
fn a_batch_that_fails_partway_accounts_for_what_it_recorded() {
	use source_sentry_core::model::RepoPath;

	use super::super::app::Flow;

	let repo = Repo::new(json!({"docs/a.md": {}}));
	repo.write("z/.source-sentry/source-sentry.yaml", "version: 1\n");
	repo.write("z/b.md", "# Nested\n");
	repo.write(
		"z/source-sentry.lock",
		r#"{"schema":1,"documents":{"b.md":{}},"retired":{}}"#,
	);
	repo.git(&["add", "--all"]);
	repo.git(&["commit", "--quiet", "-m", "Nested project"]);
	let (workspace, args) = (repo.workspace(), repo.args(&[]));
	let mut app = App::new(&workspace, &args).unwrap();
	press(&mut app, &[Key::Char('a')]);
	let Some(Overlay::All(batch)) = &app.overlay else {
		panic!("expected the confirmation");
	};
	let due: Vec<_> = batch
		.due
		.iter()
		.map(|(id, _)| id.document.to_string())
		.collect();
	assert_eq!(due, ["docs/a.md", "z/b.md"]);

	let lock = workspace.file(&RepoPath::new("z/source-sentry.lock").unwrap());
	let writable = std::fs::metadata(&lock).unwrap().permissions();
	let mut read_only = writable.clone();
	read_only.set_readonly(true);
	std::fs::set_permissions(&lock, read_only).unwrap();
	press(&mut app, &[Key::Char('r')]);
	let flow = app.key(Key::Char('y'));
	std::fs::set_permissions(&lock, writable).unwrap();

	assert_eq!(flow.unwrap(), Flow::Quit);
	let failure = app.failed.take().unwrap();
	assert!(
		failure.message.starts_with(
			"Recorded 1 review, then stopped: z/b.md was not recorded: "
		),
		"{}",
		failure.message
	);
	assert!(repo.entry("docs/a.md")["reviewed"].is_object());
	assert_eq!(app.session.steps().len(), 1);
	let closing = app.closing().plain();
	assert!(
		closing.starts_with("Recorded 1 review.\n  docs/a.md (reread)\n"),
		"{closing}"
	);
}

/// The wheel over the sidebar ends a range of picked rows, as moving the
/// cursor with a key does: the next range starts where the wheel left the
/// cursor, and the rows it passed over stay unpicked.
#[test]
fn the_wheel_ends_a_range_of_picked_rows() {
	let repo = Repo::new(json!({
		"docs/a.md": {},
		"docs/b.md": {},
		"docs/c.md": {},
		"docs/d.md": {},
		"docs/e.md": {},
	}));
	let (workspace, args) = (repo.workspace(), repo.args(&[]));
	let mut app = App::new(&workspace, &args).unwrap();
	press(&mut app, &[Key::Extend(true)]);
	assert_eq!(app.picked.len(), 2);
	press(
		&mut app,
		&[
			Key::Wheel(true, Area::Sidebar),
			Key::Wheel(true, Area::Sidebar),
		],
	);
	assert_eq!(app.cursor, Cursor::Open(3));

	press(&mut app, &[Key::Extend(true)]);
	let picked: Vec<_> = app
		.picked
		.iter()
		.map(|id| id.document.to_string())
		.collect();
	assert_eq!(picked, ["docs/a.md", "docs/b.md", "docs/d.md", "docs/e.md"]);
}

/// A drag made with Control or Shift held is left to the terminal, as a
/// click made so is, so that selecting text with Shift picks no row.
#[test]
fn a_modified_drag_is_left_to_the_terminal() {
	use ratatui::crossterm::event::{
		KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
	};

	let repo = Repo::new(json!({"docs/a.md": {}, "docs/b.md": {}}));
	let (workspace, args) = (repo.workspace(), repo.args(&[]));
	let mut app = App::new(&workspace, &args).unwrap();
	show(&mut app, 120, 30);
	let at = |modifiers| MouseEvent {
		kind: MouseEventKind::Drag(MouseButton::Left),
		column: 10,
		row: 5,
		modifiers,
	};
	for held in [KeyModifiers::SHIFT, KeyModifiers::CONTROL] {
		assert!(super::super::pointed(at(held), &app, 120).is_none());
	}
	assert!(matches!(
		super::super::pointed(at(KeyModifiers::NONE), &app, 120),
		Some(super::super::Input::Drag(Target::Row(Cursor::Open(1))))
	));
}

/// A session that finds nothing to review still ends as every session
/// does: its closing follows a final evaluation, so a change made after the
/// first one is reported.
#[test]
fn a_session_with_nothing_to_review_closes_after_a_final_evaluation() {
	let repo =
		Repo::new(json!({"docs/a.md": {"sources": [{"path": "/src/a.rs"}]}}));
	repo.attest("docs/a.md");
	let (workspace, args) = (repo.workspace(), repo.args(&[]));
	let mut app = App::new(&workspace, &args).unwrap();
	assert_eq!(app.open, Vec::new());
	repo.write("src/a.rs", "changed after the first evaluation\n");

	super::super::review(&mut app, true).unwrap();
	let closing = app.closing().plain();
	assert!(
		closing.contains("1 document still needs review:\n  docs/a.md: "),
		"{closing}"
	);
}

/// A name that no review could be recorded under, such as an email
/// address, is refused in the header: git's configuration and the session
/// keep the name they had, and the notice gives the reason.
#[test]
fn a_reviewer_name_is_checked_before_it_is_remembered() {
	let repo = Repo::new(json!({"docs/a.md": {}}));
	let (workspace, args) = (repo.workspace(), repo.args(&[]));
	let mut app = App::new(&workspace, &args).unwrap();
	let before = app.session.reviewer.clone();
	press(&mut app, &[Key::Up, Key::Enter]);
	press(&mut app, &[Key::Backspace; 64]);
	for character in "person@example.com".chars() {
		press(&mut app, &[Key::Char(character)]);
	}
	press(&mut app, &[Key::Enter]);

	let configuration = repo.git(&["config", "--local", "--list"]);
	assert!(
		!configuration.contains("person@example.com"),
		"{configuration}"
	);
	assert_eq!(app.session.reviewer, before);
	let notice = app.notice.clone().unwrap_or_default();
	assert!(notice.contains("never email addresses"), "{notice}");
}

/// An answer given to several documents writes their lockfile once for
/// each. Another program that changes the lockfile between two of those
/// writes leaves the batch unable to restore it: taking the batch back would
/// erase that change too, so the answer is kept and the refusal says so,
/// naming the file.
#[test]
fn a_change_between_the_writes_of_a_batch_refuses_taking_it_back() {
	use super::super::super::session::Undone;

	let repo =
		Repo::new(json!({"docs/a.md": {}, "docs/b.md": {}, "docs/c.md": {}}));
	let (workspace, args) = (repo.workspace(), repo.args(&[]));
	let mut app = App::new(&workspace, &args).unwrap();
	let [a, b] = [0, 1].map(|index| app.open[index].clone());
	assert!(a.document.to_string().ends_with("a.md"));
	assert!(b.document.to_string().ends_with("b.md"));
	let first = app.session.compare(&a).unwrap();
	let second = app.session.compare(&b).unwrap();

	assert!(app.session.write(&a, &first, false).is_ok());
	repo.attest("docs/c.md");
	let between = repo.entry("docs/c.md");
	assert!(between["reviewed"].is_object());
	assert!(app.session.write(&b, &second, false).is_ok());
	app.session.join(0);
	assert_eq!(app.session.steps().len(), 1);
	let after = repo.read("source-sentry.lock");

	let Undone::Interleaved(path) = app.session.undo().unwrap() else {
		panic!("the batch was taken back, or refused as changed since");
	};
	assert_eq!(path, "source-sentry.lock");
	assert_eq!(repo.read("source-sentry.lock"), after);
	assert_eq!(repo.entry("docs/c.md"), between);
	assert_eq!(app.session.steps().len(), 1);
}

/// A comparison is seen as it was shown: from the reviewed version to the
/// current one. When the reviewed version changes outside the screen, the
/// comparison is a new one, and counts as unseen until it is shown, even
/// though the current version is the same.
#[test]
fn a_comparison_from_another_reviewed_version_is_unseen() {
	let repo =
		Repo::new(json!({"docs/a.md": {"sources": [{"path": "/src/a.rs"}]}}));
	repo.attest("docs/a.md");
	repo.write("src/a.rs", "second a\n");
	repo.git(&["commit", "--quiet", "--all", "-m", "Second"]);
	repo.write("src/a.rs", "third a\n");
	let (workspace, args) = (repo.workspace(), repo.args(&[]));
	let mut app = App::new(&workspace, &args).unwrap();
	let a = app.open[0].clone();
	let shown = app.session.compare(&a).unwrap();
	app.session.see(&a, &shown);
	assert!(app.session.has_seen(&a, &shown));

	// Another program records a review of the second version.
	repo.write("src/a.rs", "second a\n");
	repo.attest("docs/a.md");
	repo.write("src/a.rs", "third a\n");
	app.session.refresh().unwrap();
	let now = app.session.compare(&a).unwrap();
	assert_eq!(now.expectations, shown.expectations);
	assert_ne!(now, shown);
	assert!(!app.session.has_seen(&a, &now));
	assert_eq!(app.session.unseen(&[(a.clone(), now.clone())]), 1);
	app.session.see(&a, &now);
	assert!(app.session.has_seen(&a, &now));
}

/// A document reviewed whole that still waits on an upstream document
/// moves to the other documents, as the line mode stops asking about it:
/// another review would add nothing until its evidence changes.
#[test]
fn a_reviewed_document_waiting_on_an_upstream_one_leaves_the_queue() {
	let repo = Repo::new(json!({
		"docs/a.md": {"sources": [{"path": "/src/a.rs"}, {"path": "up.md"}]},
		"docs/up.md": {"sources": [{"path": "/src/b.rs"}]},
	}));
	repo.attest("docs/up.md");
	repo.attest("docs/a.md");
	repo.git(&["add", "--all"]);
	repo.git(&["commit", "--quiet", "-m", "Attest"]);
	repo.write("src/b.rs", "changed upstream\n");
	repo.write("src/a.rs", "changed\n");
	let (workspace, args) = (repo.workspace(), repo.args(&["docs/a.md"]));
	let mut app = App::new(&workspace, &args).unwrap();
	assert_eq!(app.open.len(), 1);

	press(&mut app, &[Key::Char('y'), Key::Enter]);
	assert_eq!(app.session.recorded.len(), 1);
	assert_eq!(app.open, Vec::new());
	assert!(
		app
			.others
			.iter()
			.any(|other| other.id.document.to_string() == "docs/a.md")
	);
}

/// A document whose reread is all it needs holds a reread on a plain `y`,
/// since nothing else is left to confirm. `a` names it as a reread and
/// records it only once `r` includes the rereads.
#[test]
fn a_plain_yes_rereads_a_document_that_needs_only_its_reread() {
	let schedule =
		json!({"reread": {"dtstart": "2026-01-01", "rrule": "FREQ=MONTHLY"}});
	let repo = Repo::new(json!({
		"docs/a.md": {"schedule": schedule, "sources": [{"path": "/src/a.rs"}]},
	}));
	repo.attest("docs/a.md");
	repo.git(&["add", "--all"]);
	repo.git(&["commit", "--quiet", "-m", "Reread in October"]);
	let earlier = repo.entry("docs/a.md")["reviewed"]["at"].clone();
	let (workspace, args) = (repo.workspace(), repo.args(&[]));
	let mut app = App::new(&workspace, &args).unwrap();
	assert_eq!(app.open.len(), 1);

	press(&mut app, &[Key::Char('a')]);
	let Some(Overlay::All(batch)) = &app.overlay else {
		panic!("expected the confirmation");
	};
	assert_eq!((batch.due.len(), batch.rereads.len()), (1, 1));
	let screen = drawn(&app, 120, 30);
	assert!(
		shows(
			&screen,
			"Every document left can only be recorded as reread."
		),
		"{screen:#?}"
	);
	assert!(shows(&screen, "left out"), "{screen:#?}");
	// Nothing it would take is due until the rereads are included.
	press(&mut app, &[Key::Char('y')]);
	assert!(matches!(app.overlay, Some(Overlay::All(_))));
	press(&mut app, &[Key::Char('r')]);
	assert!(shows(
		&drawn(&app, 120, 30),
		"Recorded as reread, each read whole"
	));
	press(&mut app, &[Key::Char('n')]);
	assert_eq!(repo.entry("docs/a.md")["reviewed"]["at"], earlier);

	press(&mut app, &[Key::Char('y')]);
	assert_eq!(app.armed, Some(true));
	assert!(shows(&drawn(&app, 120, 30), "reread in full"));
	press(&mut app, &[Key::Enter]);
	assert_ne!(repo.entry("docs/a.md")["reviewed"]["at"], earlier);
	assert_eq!(app.open, Vec::new());
}

/// `a` records the documents without sources, which can only take a
/// reread, only once `r` includes them, and records each as reread.
#[test]
fn all_records_documents_without_sources_only_when_included() {
	let repo = Repo::new(json!({
		"docs/a.md": {},
		"docs/c.md": {"sources": [{"path": "/src/a.rs"}]},
	}));
	let (workspace, args) = (repo.workspace(), repo.args(&[]));
	let mut app = App::new(&workspace, &args).unwrap();
	press(&mut app, &[Key::Char('a'), Key::Char('y')]);
	assert!(repo.entry("docs/a.md")["reviewed"].is_null());
	assert!(repo.entry("docs/c.md")["sources"][0]["reviewed"].is_object());
	assert_eq!(app.notice.as_deref(), Some("Recorded 1 review."));

	press(&mut app, &[Key::Char('a'), Key::Char('r'), Key::Char('y')]);
	assert!(repo.entry("docs/a.md")["reviewed"].is_object());
	assert_eq!(
		app.session.recorded.last().map(|(_, reread)| *reread),
		Some(true)
	);
}
