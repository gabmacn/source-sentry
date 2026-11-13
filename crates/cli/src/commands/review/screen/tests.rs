//! Scripted keys and drawn grids against real temporary repositories.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use serde_json::json;

use super::super::tests::support::Repo;
use std::rc::Rc;

use super::app::{
	App, Area, Cursor, Field, Flow, Focus, Key, Kind, Overlay, Target,
};
use super::draw;

/// A repository with two documents that need review, the first with a
/// source, and one that needs nothing.
fn fixture() -> Repo {
	let repo = Repo::new(
		json!({"docs/a.md": {"sources": [{"path": "/src/a.rs"}]}, "docs/b.md": {}, "docs/fresh.md": {}}),
	);
	repo.attest("docs/fresh.md");
	repo
}

/// Presses keys that must all leave the session running.
fn press(app: &mut App, keys: &[Key]) {
	for key in keys {
		assert_eq!(app.key(*key).unwrap(), Flow::Stay, "{key:?}");
	}
}

/// The screen at a size, as its rows of text.
fn drawn(app: &App, columns: u16, rows: u16) -> Vec<String> {
	let mut buffer = Buffer::empty(Rect::new(0, 0, columns, rows));
	draw::draw(app, &mut buffer);
	(0..rows)
		.map(|y| {
			(0..columns)
				.map(|x| buffer[(x, y)].symbol())
				.collect::<String>()
				.trim_end()
				.to_owned()
		})
		.collect()
}

/// Whether any row of the screen holds `text`.
fn shows(rows: &[String], text: &str) -> bool {
	rows.iter().any(|row| row.contains(text))
}

/// `y` holds the answer; only Enter writes it. The reviewed document then
/// leaves the queue for the other documents, and the next one is shown.
#[test]
fn an_answer_is_recorded_only_after_enter_and_the_queue_moves_on() {
	let repo = fixture();
	let (workspace, args) = (repo.workspace(), repo.args(&[]));
	let mut app = App::new(&workspace, &args).unwrap();
	assert_eq!((app.open.len(), app.cursor), (2, Cursor::Open(0)));
	assert_eq!(app.progress(), (0, 2));

	press(&mut app, &[Key::Char('y')]);
	assert_eq!(app.armed, Some(false));
	assert!(repo.entry("docs/a.md")["sources"][0]["reviewed"].is_null());
	press(&mut app, &[Key::Down]);
	assert_eq!((app.armed, app.cursor), (None, Cursor::Open(1)));
	press(&mut app, &[Key::Enter]);
	assert!(repo.entry("docs/a.md")["sources"][0]["reviewed"].is_null());

	press(&mut app, &[Key::Up, Key::Char('y'), Key::Enter]);
	assert_eq!(
		repo.entry("docs/a.md")["sources"][0]["reviewed"]["by"],
		"Test Reviewer"
	);
	assert_eq!((app.open.len(), app.cursor), (1, Cursor::Open(0)));
	assert_eq!(app.others[0].kind, Kind::Reviewed);
	assert_eq!(app.progress(), (1, 2));
	assert!(app.shown.as_ref().unwrap().name.ends_with("b.md"));
	assert!(app.notice.as_ref().unwrap().starts_with("Recorded "));
}

/// Recording everything asks first, names what was not seen, and writes
/// nothing unless the answer is yes.
#[test]
fn everything_is_recorded_at_once_only_after_a_confirmation() {
	let repo = fixture();
	let (workspace, args) = (repo.workspace(), repo.args(&[]));
	let mut app = App::new(&workspace, &args).unwrap();
	press(&mut app, &[Key::Char('a')]);
	let Some(Overlay::All(comparisons, unseen)) = &app.overlay else {
		panic!("expected the confirmation");
	};
	assert_eq!((comparisons.len(), *unseen), (2, 1));
	assert!(shows(
		&drawn(&app, 120, 30),
		"Record the remaining 2 documents, including 1 you have not seen?"
	));
	press(&mut app, &[Key::Enter]);
	assert_eq!((app.overlay.is_none(), app.open.len()), (true, 2));
	assert!(repo.entry("docs/b.md")["reviewed"].is_null());

	press(&mut app, &[Key::Char('a'), Key::Char('y')]);
	assert_eq!((app.open.len(), app.cursor), (0, Cursor::None));
	assert!(repo.entry("docs/b.md")["reviewed"].is_object());
	assert_eq!(app.notice.as_deref(), Some("Recorded 2 reviews."));
	assert!(shows(&drawn(&app, 120, 30), "Nothing is left to review."));
	assert_eq!(app.key(Key::Char('q')).unwrap(), Flow::Quit);
	assert!(
		app
			.closing()
			.plain()
			.contains("0 documents still need review")
	);
}

/// A document that needs nothing is opened from the other documents. It
/// takes a reread and stays under the cursor, but there is no yes to give.
#[test]
fn a_passing_document_takes_a_reread_but_no_yes() {
	let repo = fixture();
	let (workspace, args) = (repo.workspace(), repo.args(&[]));
	let mut app = App::new(&workspace, &args).unwrap();
	press(&mut app, &[Key::Char('o'), Key::Down, Key::Down, Key::Down]);
	assert_eq!(app.cursor, Cursor::Other(0));
	assert!(app.shown.as_ref().unwrap().name.ends_with("fresh.md"));
	assert!(shows(
		&drawn(&app, 120, 30),
		"Nothing changed since the last review."
	));
	press(&mut app, &[Key::Char('y')]);
	assert_eq!(app.armed, None);
	assert!(app.notice.as_ref().unwrap().contains("r records a reread"));
	press(&mut app, &[Key::Char('r'), Key::Enter]);
	assert_eq!(app.session.recorded.len(), 1);
	assert!(app.session.recorded[0].1);
	assert_eq!(app.cursor, Cursor::Other(0));
	assert_eq!(app.others[0].kind, Kind::Reviewed);
	assert_eq!(app.open.len(), 2);
}

/// Above the first document is the header: the reviewer's name is typed
/// there and the context chosen, and later reviews carry the new name.
#[test]
fn the_reviewer_and_the_context_change_in_the_header() {
	let repo = fixture();
	let (workspace, args) = (repo.workspace(), repo.args(&[]));
	let mut app = App::new(&workspace, &args).unwrap();
	press(&mut app, &[Key::Up, Key::Enter]);
	assert_eq!(
		app.overlay,
		Some(Overlay::Reviewer("Test Reviewer".to_owned()))
	);
	press(&mut app, &[Key::Backspace; 13]);
	press(
		&mut app,
		&[Key::Char('A'), Key::Char('n'), Key::Char('n'), Key::Enter],
	);
	assert_eq!(app.session.reviewer.as_deref(), Some("Ann"));
	assert_eq!(
		repo
			.git(&["config", "--get", "source-sentry.reviewer"])
			.trim(),
		"Ann"
	);
	let later = App::new(&workspace, &args).unwrap();
	assert_eq!(later.session.reviewer.as_deref(), Some("Ann"));

	press(&mut app, &[Key::Right, Key::Enter]);
	assert!(matches!(app.overlay, Some(Overlay::Contexts(_))));
	press(&mut app, &[Key::Enter]);
	assert_eq!(app.cursor, Cursor::Header(Field::Context));
	assert_eq!(app.session.context, "default");

	press(&mut app, &[Key::Down, Key::Char('y'), Key::Enter]);
	assert_eq!(
		repo.entry("docs/a.md")["sources"][0]["reviewed"]["by"],
		"Ann"
	);

	// An empty name forgets the remembered one and returns to git's own.
	let mut app = App::new(&workspace, &args).unwrap();
	press(&mut app, &[Key::Up, Key::Enter]);
	press(&mut app, &[Key::Backspace; 3]);
	press(&mut app, &[Key::Enter]);
	assert_eq!(app.session.reviewer.as_deref(), Some("Test Reviewer"));
	assert!(
		!repo
			.git(&["config", "--local", "--list"])
			.contains("source-sentry.reviewer")
	);
}

/// The screen holds the session's facts, the queue, the mark, the changes
/// of the document on display and the question, and swaps the pane on `v`.
#[test]
fn the_screen_shows_the_queue_the_changes_and_the_question() {
	let repo = fixture();
	let (workspace, args) = (repo.workspace(), repo.args(&[]));
	let mut app = App::new(&workspace, &args).unwrap();
	let rows = drawn(&app, 120, 30);
	assert!(rows[0].starts_with(" SOURCE SENTRY "), "{}", rows[0]);
	assert!(rows[0].contains("Reviewer Test Reviewer"), "{}", rows[0]);
	assert!(rows[0].contains("0 of 2 reviewed"), "{}", rows[0]);
	assert!(rows[2].starts_with(" NEEDS REVIEW"), "{}", rows[2]);
	for text in [
		"OTHER DOCUMENTS",
		"▄▄██████▄▄",
		"1 changed of 1",
		"   1 + old a",
		"unattested →",
	] {
		assert!(shows(&rows, text), "{text}:\n{}", rows.join("\n"));
	}
	assert!(rows[28].contains("accurate?"), "{}", rows[28]);
	let narrow = drawn(&app, 80, 24);
	assert!(narrow[22].starts_with(" Still accurate?"), "{}", narrow[22]);
	assert!(narrow[22].ends_with(" a  yes to all"), "{}", narrow[22]);
	assert!(narrow[23].contains("? keys   q quit"), "{}", narrow[23]);
	assert!(rows[28].contains(" y  yes"), "{}", rows[28]);
	assert!(rows[29].contains("? keys"), "{}", rows[29]);

	press(&mut app, &[Key::Char('v')]);
	let rows = drawn(&app, 120, 30);
	assert!(shows(&rows, "THE DOCUMENT") && shows(&rows, "1  # Document"));
	assert!(!shows(&rows, "SOURCES"));

	let wide = drawn(&app, 210, 44);
	assert!(shows(&wide, "SOURCES") && shows(&wide, "THE DOCUMENT"));
	let small = drawn(&app, 60, 12);
	assert!(
		shows(&small, "needs 80 by 20 cells"),
		"{}",
		small.join("\n")
	);

	press(&mut app, &[Key::Char('?')]);
	assert!(shows(&drawn(&app, 120, 30), "show its whole path"));
}

/// A document is read once per evaluation and ahead of the cursor. One that
/// was only read ahead has not been seen.
#[test]
fn documents_are_read_once_and_ahead_without_counting_as_seen() {
	let repo = fixture();
	let (workspace, args) = (repo.workspace(), repo.args(&[]));
	let mut app = App::new(&workspace, &args).unwrap();
	assert_eq!(app.cache.len(), 1);
	assert!(app.prefetch());
	assert!(!app.prefetch());
	assert_eq!(app.cache.len(), 2);
	press(&mut app, &[Key::Char('a')]);
	assert!(matches!(app.overlay, Some(Overlay::All(_, 1))));

	press(&mut app, &[Key::Escape]);
	let first = Rc::clone(app.shown.as_ref().unwrap());
	press(&mut app, &[Key::Down, Key::Up]);
	assert!(Rc::ptr_eq(&first, app.shown.as_ref().unwrap()));
	press(&mut app, &[Key::Char('a')]);
	assert!(matches!(app.overlay, Some(Overlay::All(_, 0))));

	press(&mut app, &[Key::Escape, Key::Char('o')]);
	assert!(app.prefetch());
	assert!(!app.prefetch());
	assert_eq!(app.cache.len(), 3);
}

/// The wheel scrolls the pane it is over and moves the cursor over the
/// sidebar. Over a pane it keeps a held answer, and it never closes a box.
#[test]
fn the_wheel_scrolls_the_pane_under_it() {
	let repo = fixture();
	repo.write("src/a.rs", &"line\n".repeat(40));
	let (workspace, args) = (repo.workspace(), repo.args(&[]));
	let mut app = App::new(&workspace, &args).unwrap();
	press(&mut app, &[Key::Char('y'), Key::Wheel(true, Area::Pane)]);
	assert_eq!((app.scroll, app.armed), ((3, 0), Some(false)));
	press(&mut app, &[Key::Wheel(true, Area::Document)]);
	assert_eq!(app.scroll, (3, 0), "the document has a single line");
	press(&mut app, &[Key::Wheel(false, Area::Changes); 2]);
	assert_eq!(app.scroll, (0, 0));
	press(&mut app, &[Key::Wheel(true, Area::Sidebar)]);
	assert_eq!((app.cursor, app.armed), (Cursor::Open(1), None));
	press(&mut app, &[Key::Char('?'), Key::Wheel(true, Area::Pane)]);
	assert_eq!(app.overlay, Some(Overlay::Keys));
	assert_eq!(draw::area_at(120, 10), Area::Sidebar);
	assert_eq!(draw::area_at(120, 60), Area::Pane);
	assert_eq!(draw::area_at(210, 60), Area::Changes);
	assert_eq!(draw::area_at(210, 180), Area::Document);
}

/// A box clears what it covers: the weight of the text behind it does not
/// show through in the box's own text.
#[test]
fn a_box_does_not_inherit_the_styles_it_covers() {
	let repo = fixture();
	repo.write("src/a.rs", &"line\n".repeat(40));
	let (workspace, args) = (repo.workspace(), repo.args(&[]));
	let mut app = App::new(&workspace, &args).unwrap();
	press(&mut app, &[Key::Up, Key::Right, Key::Right, Key::Enter]);
	assert_eq!(app.overlay, Some(Overlay::Projects));
	let mut buffer = Buffer::empty(Rect::new(0, 0, 120, 30));
	draw::draw(&app, &mut buffer);
	// The rows of the box between its title and its keys, over the diff's
	// bold signs.
	for y in 12..=15 {
		for x in 30..90 {
			let cell = &buffer[(x, y)];
			assert!(cell.modifier.is_empty(), "({x}, {y}): {cell:?}");
		}
	}
}

/// The arrow keys and Tab move in the list that has the focus. From a
/// document, the right arrow or Enter goes to its changed sources, where
/// Enter shows a whole path; the left arrow or Escape returns.
#[test]
fn the_focus_moves_between_the_documents_and_their_sources() {
	let repo = Repo::new(
		json!({"docs/a.md": {"sources": [{"path": "/src/a.rs"}, {"path": "/src/b.rs"}]}, "docs/b.md": {}}),
	);
	let (workspace, args) = (repo.workspace(), repo.args(&[]));
	let mut app = App::new(&workspace, &args).unwrap();
	assert_eq!((app.focus, app.cursor), (Focus::Sidebar, Cursor::Open(0)));
	press(&mut app, &[Key::Tab]);
	assert_eq!(app.cursor, Cursor::Open(1));
	press(&mut app, &[Key::Right]);
	assert_eq!(app.focus, Focus::Sidebar, "no changed source to go to");
	assert!(app.notice.is_some());
	press(&mut app, &[Key::BackTab, Key::Enter]);
	assert_eq!(
		(app.focus, app.cursor, app.file),
		(Focus::Sources, Cursor::Open(0), 0)
	);

	let rows = drawn(&app, 120, 30);
	assert!(rows[29].contains("Enter whole path"), "{}", rows[29]);
	let mut buffer = Buffer::empty(Rect::new(0, 0, 120, 30));
	draw::draw(&app, &mut buffer);
	assert_eq!(
		buffer[(1, 3)].bg,
		draw::MUTED_BLUE,
		"the document steps back"
	);

	press(&mut app, &[Key::Down, Key::Down]);
	assert_eq!((app.file, app.cursor), (1, Cursor::Open(0)));
	press(&mut app, &[Key::Tab]);
	assert_eq!(app.file, 0);
	press(&mut app, &[Key::Enter]);
	let Some(Overlay::Path(path)) = &app.overlay else {
		panic!("expected the whole path");
	};
	assert!(path.ends_with("a.rs"));
	press(&mut app, &[Key::Escape]);
	assert_eq!((app.overlay.is_none(), app.focus), (true, Focus::Sources));
	press(&mut app, &[Key::Char('y'), Key::Escape]);
	assert_eq!((app.armed, app.focus), (None, Focus::Sources));
	press(&mut app, &[Key::Left]);
	assert_eq!(app.focus, Focus::Sidebar);
}

/// The label of the other documents is a row of its own: Enter on it opens
/// and closes them, as `o` does anywhere.
#[test]
fn the_label_of_the_other_documents_opens_and_closes_them() {
	let repo = fixture();
	let (workspace, args) = (repo.workspace(), repo.args(&[]));
	let mut app = App::new(&workspace, &args).unwrap();
	press(&mut app, &[Key::Down, Key::Down]);
	assert_eq!(app.cursor, Cursor::Others);
	assert!(app.shown.as_ref().unwrap().name.ends_with("b.md"));
	press(&mut app, &[Key::Char('y')]);
	assert_eq!(app.armed, None, "the label is not a document");
	press(&mut app, &[Key::Down]);
	assert_eq!(app.cursor, Cursor::Others, "nothing is listed below it yet");
	press(&mut app, &[Key::Enter]);
	assert!(app.expanded);
	press(&mut app, &[Key::Down]);
	assert_eq!(app.cursor, Cursor::Other(0));
	press(&mut app, &[Key::Char('o')]);
	assert_eq!((app.expanded, app.cursor), (false, Cursor::Others));
	press(&mut app, &[Key::Tab]);
	assert_eq!(app.cursor, Cursor::Open(0), "Tab goes round");
}

/// Draws the screen and keeps what a click acts on, as the terminal loop
/// does.
fn show(app: &mut App, columns: u16, rows: u16) -> Buffer {
	let mut buffer = Buffer::empty(Rect::new(0, 0, columns, rows));
	app.hits = draw::draw(app, &mut buffer);
	buffer
}

/// A click does what the matching key does: it selects a row, opens the
/// other documents or a header field, presses a cap. A second click on a
/// row acts as Enter does there, but never records a held answer.
#[test]
fn a_click_does_what_the_matching_key_does() {
	let repo = Repo::new(
		json!({"docs/a.md": {"sources": [{"path": "/src/a.rs"}, {"path": "/src/b.rs"}]}, "docs/b.md": {}}),
	);
	let (workspace, args) = (repo.workspace(), repo.args(&[]));
	let mut app = App::new(&workspace, &args).unwrap();
	show(&mut app, 120, 30);
	assert_eq!(app.target_at(10, 5), Some(Target::Row(Cursor::Open(1))));
	assert_eq!(app.target_at(60, 15), None, "a diff line takes no click");
	app.click(Target::Row(Cursor::Open(1))).unwrap();
	assert_eq!(app.cursor, Cursor::Open(1));
	app.click(Target::Row(Cursor::Open(0))).unwrap();
	assert_eq!((app.cursor, app.focus), (Cursor::Open(0), Focus::Sidebar));

	press(&mut app, &[Key::Char('y')]);
	app.click(Target::Row(Cursor::Open(0))).unwrap();
	assert_eq!((app.armed, app.focus), (None, Focus::Sources));
	assert!(repo.entry("docs/a.md")["sources"][0]["reviewed"].is_null());
	app.click(Target::Source(1)).unwrap();
	assert_eq!((app.file, app.focus), (1, Focus::Sources));
	app.click(Target::Source(1)).unwrap();
	assert!(matches!(app.overlay, Some(Overlay::Path(_))));

	// Under a box, only the box takes clicks. One on the box itself does
	// nothing, and one around it closes it.
	show(&mut app, 120, 30);
	assert_eq!(app.target_at(10, 5), Some(Target::Outside));
	let inside = app
		.hits
		.iter()
		.find(|(_, target)| *target == Target::Box)
		.map(|(area, _)| *area)
		.unwrap();
	assert_eq!(app.target_at(inside.x + 1, inside.y + 1), Some(Target::Box));
	app.click(Target::Box).unwrap();
	assert!(app.overlay.is_some());
	app.click(Target::Outside).unwrap();
	assert!(app.overlay.is_none());
	assert_eq!(app.focus, Focus::Sources, "only the box closed");
	app.click(Target::Source(1)).unwrap();
	assert!(matches!(app.overlay, Some(Overlay::Path(_))));
	show(&mut app, 120, 30);
	let close = app
		.hits
		.iter()
		.find(|(_, target)| *target == Target::Cap(Key::Escape))
		.map(|(area, _)| *area)
		.unwrap();
	let target = app.target_at(close.x, close.y).unwrap();
	app.click(target).unwrap();
	assert!(app.overlay.is_none());

	app.click(Target::Row(Cursor::Others)).unwrap();
	assert_eq!((app.expanded, app.cursor), (true, Cursor::Others));
	app.click(Target::Field(Field::Reviewer)).unwrap();
	assert!(matches!(app.overlay, Some(Overlay::Reviewer(_))));
	press(&mut app, &[Key::Escape]);

	app.click(Target::Row(Cursor::Open(0))).unwrap();
	app.click(Target::Cap(Key::Char('y'))).unwrap();
	assert_eq!(app.armed, Some(false));
	show(&mut app, 120, 30);
	assert!(
		app
			.hits
			.iter()
			.any(|(_, target)| *target == Target::Cap(Key::Enter))
	);
	app.click(Target::Cap(Key::Enter)).unwrap();
	assert!(repo.entry("docs/a.md")["sources"][0]["reviewed"].is_object());
}

/// The pointer lightly marks what a click would act on, and nothing else.
#[test]
fn the_pointer_marks_what_a_click_would_act_on() {
	let repo = fixture();
	let (workspace, args) = (repo.workspace(), repo.args(&[]));
	let mut app = App::new(&workspace, &args).unwrap();
	show(&mut app, 120, 30);
	app.pointer = Some((10, 5));
	assert!(app.hover_changed());
	assert_eq!(app.hover, Some(Target::Row(Cursor::Open(1))));
	assert!(!app.hover_changed());
	let buffer = show(&mut app, 120, 30);
	assert_eq!(buffer[(10, 5)].bg, draw::HOVER);
	assert_eq!(buffer[(10, 6)].bg, draw::HOVER);

	app.pointer = Some((10, 3));
	assert!(app.hover_changed());
	let buffer = show(&mut app, 120, 30);
	assert_ne!(
		buffer[(10, 3)].bg,
		draw::HOVER,
		"the selection keeps its mark"
	);
	assert_ne!(buffer[(10, 5)].bg, draw::HOVER);

	app.pointer = Some((60, 15));
	assert!(app.hover_changed());
	assert_eq!(app.hover, None);
}
