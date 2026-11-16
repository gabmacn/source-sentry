//! What an answer records: exactly what the reviewer was shown, for the
//! document on display.

use serde_json::json;

use super::super::super::session::CHANGED;
use super::super::super::tests::support::Repo;
use super::super::app::{App, Cursor, Focus, Key, Target};
use super::{drawn, press, shows};

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
	press(&mut app, &[Key::Char('y')]);
	assert_eq!(app.armed, Some(false));

	app.drag(Target::Row(Cursor::Open(1)));
	assert!(app.shown.as_ref().unwrap().name.ends_with("b.md"));
	assert_eq!((app.armed, app.picked.len()), (None, 2));
	press(&mut app, &[Key::Enter]);
	assert_eq!(app.session.recorded, Vec::new());
	assert!(repo.entry("docs/a.md")["reviewed"].is_null());
	assert!(repo.entry("docs/b.md")["reviewed"].is_null());
}
