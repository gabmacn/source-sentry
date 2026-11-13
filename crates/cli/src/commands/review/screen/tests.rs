//! Scripted keys and drawn grids against real temporary repositories.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use serde_json::json;

use super::super::tests::support::Repo;
use std::rc::Rc;

use super::app::{
	App, Area, Cursor, Field, Flow, Focus, Kept, Key, Kind, Overlay, Target,
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

	press(&mut app, &[Key::Left, Key::Up, Key::Char('y'), Key::Enter]);
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
	// The buttons of the last row: as many as fit whole. Where the swap's
	// own name would cost one, a shorter name stands for it.
	assert_eq!(
		narrow[23],
		"  ?  keys   q  quit   v  view   s  sources   u  undo   h  history   Space  pick"
	);
	assert!(rows[28].contains(" y  yes"), "{}", rows[28]);
	assert!(
		rows[29].contains(" v  document   s  sources"),
		"{}",
		rows[29]
	);
	assert!(
		rows[29].ends_with(" Space  pick   o  others"),
		"{}",
		rows[29]
	);
	// On a narrow screen the progress gives way to the header's fields.
	assert!(narrow[0].contains("Projects 1  "), "{}", narrow[0]);
	assert!(narrow[0].ends_with(" 0/2"), "{}", narrow[0]);

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

	// The other documents are many and seldom opened: none is read as
	// their list opens, only the one the cursor reaches.
	press(&mut app, &[Key::Escape, Key::Char('o')]);
	assert!(!app.prefetch());
	assert_eq!(app.cache.len(), 2);
	press(&mut app, &[Key::Down, Key::Down, Key::Down]);
	assert_eq!((app.cursor, app.cache.len()), (Cursor::Other(0), 3));
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
	assert_eq!(app.focus, Focus::Title, "no changed source: its name");
	press(&mut app, &[Key::Left, Key::BackTab, Key::Enter]);
	assert_eq!(
		(app.focus, app.cursor, app.file),
		(Focus::Sources, Cursor::Open(0), 0)
	);

	let rows = drawn(&app, 120, 30);
	assert!(rows[29].contains(" Enter  whole path"), "{}", rows[29]);
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

/// A character typed with Control or Alt is not taken for the plain
/// letter, so Control and A does not ask to record everything. Control and
/// C still quits, and Control with Alt, which is `AltGr` on Windows, types.
#[test]
fn shortcuts_the_screen_does_not_have_are_ignored() {
	use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
	let typed = |character, modifiers| {
		super::translate(KeyEvent::new(KeyCode::Char(character), modifiers))
	};
	assert_eq!(typed('a', KeyModifiers::CONTROL), None);
	assert_eq!(typed('y', KeyModifiers::ALT), None);
	assert_eq!(typed('c', KeyModifiers::CONTROL), Some(Key::Interrupt));
	assert_eq!(typed('y', KeyModifiers::NONE), Some(Key::Char('y')));
	assert_eq!(typed('Y', KeyModifiers::SHIFT), Some(Key::Char('Y')));
	assert_eq!(
		typed('@', KeyModifiers::CONTROL | KeyModifiers::ALT),
		Some(Key::Char('@'))
	);
}

/// The wheel passes over documents in the sidebar without seeing them; a
/// document it brings counts as seen once the reviewer acts while it is
/// shown.
#[test]
fn the_wheel_does_not_count_a_document_as_seen() {
	let repo = fixture();
	let (workspace, args) = (repo.workspace(), repo.args(&[]));
	let mut app = App::new(&workspace, &args).unwrap();
	press(
		&mut app,
		&[
			Key::Wheel(true, Area::Sidebar),
			Key::Wheel(false, Area::Sidebar),
		],
	);
	assert_eq!(app.cursor, Cursor::Open(0));
	press(&mut app, &[Key::Char('a')]);
	assert!(matches!(app.overlay, Some(Overlay::All(_, 1))));

	press(&mut app, &[Key::Escape, Key::Wheel(true, Area::Sidebar)]);
	assert_eq!(app.cursor, Cursor::Open(1));
	press(&mut app, &[Key::Char('a')]);
	assert!(matches!(app.overlay, Some(Overlay::All(_, 0))));
}

/// A source changed after the document was shown refuses the held answer:
/// nothing is written, and the document is shown again with its current
/// changes.
#[test]
fn a_source_changed_after_it_was_shown_refuses_the_answer() {
	let repo = fixture();
	let (workspace, args) = (repo.workspace(), repo.args(&[]));
	let mut app = App::new(&workspace, &args).unwrap();
	let before = app.shown.as_ref().unwrap().compared.clone().unwrap();
	repo.write("src/a.rs", "changed while shown\n");
	press(&mut app, &[Key::Char('y'), Key::Enter]);
	assert!(repo.entry("docs/a.md")["sources"][0]["reviewed"].is_null());
	assert_eq!(app.session.recorded, Vec::new());
	assert_eq!(app.notice.as_deref(), Some(super::super::session::CHANGED));
	let shown = app.shown.as_ref().unwrap();
	assert!(shown.name.ends_with("a.md"));
	assert_ne!(
		shown.compared.as_ref().unwrap().expectations,
		before.expectations
	);

	press(&mut app, &[Key::Char('y'), Key::Enter]);
	assert_eq!(
		repo.entry("docs/a.md")["sources"][0]["reviewed"]["by"],
		"Test Reviewer"
	);
}

/// The closing evaluates once more: a change made outside the screen after
/// the last write is reported, and the part known at once comes first.
#[test]
fn the_closing_reports_changes_made_after_the_last_write() {
	use super::super::closing::Part;
	let repo = fixture();
	let (workspace, args) = (repo.workspace(), repo.args(&[]));
	let mut app = App::new(&workspace, &args).unwrap();
	press(&mut app, &[Key::Char('y'), Key::Enter]);
	assert!(!app.closing().plain().contains("docs/a.md: "));
	repo.write("src/a.rs", "changed outside the screen\n");
	let mut parts = Vec::new();
	app
		.session
		.finish(|closing, part| parts.push((part, closing.part(part, true))))
		.unwrap();
	assert_eq!(
		parts[0],
		(Part::Known, "Recorded 1 review.\n  docs/a.md".to_owned())
	);
	assert_eq!(parts[1].0, Part::Evaluated);
	assert!(parts[1].1.contains("docs/a.md: "), "{}", parts[1].1);
	assert!(
		parts[1].1.starts_with("2 documents still need review"),
		"{}",
		parts[1].1
	);
}

/// Several documents are picked with the space bar or with Shift and an
/// arrow, and take one answer after a confirmation that lists them.
#[test]
fn picked_documents_take_one_answer_after_a_confirmation() {
	let repo = fixture();
	let (workspace, args) = (repo.workspace(), repo.args(&[]));
	let mut app = App::new(&workspace, &args).unwrap();
	press(&mut app, &[Key::Extend(true)]);
	assert_eq!((app.picked.len(), app.cursor), (2, Cursor::Open(1)));
	press(&mut app, &[Key::Extend(false)]);
	assert_eq!((app.picked.len(), app.cursor), (1, Cursor::Open(0)));
	press(&mut app, &[Key::Escape]);
	assert!(app.picked.is_empty());

	press(&mut app, &[Key::Char(' '), Key::Down, Key::Char(' ')]);
	assert_eq!(app.picked.len(), 2);
	let rows = drawn(&app, 210, 44);
	assert!(shows(&rows, "Are the 2 picked documents still accurate?"));
	assert!(rows[3].starts_with('▌') && rows[5].starts_with('▌'));

	press(&mut app, &[Key::Char('y')]);
	let Some(Overlay::Picked(comparisons, unseen, reread)) = &app.overlay else {
		panic!("expected the confirmation");
	};
	assert_eq!((comparisons.len(), *unseen, *reread), (2, 0, false));
	assert!(shows(
		&drawn(&app, 120, 30),
		"Record the 2 picked documents, including 0 you have not seen?"
	));
	press(&mut app, &[Key::Char('n')]);
	assert_eq!(app.picked.len(), 2);
	assert!(repo.entry("docs/b.md")["reviewed"].is_null());

	press(&mut app, &[Key::Char('y'), Key::Char('y')]);
	assert!(app.picked.is_empty() && app.open.is_empty());
	assert!(repo.entry("docs/b.md")["reviewed"].is_object());
	assert_eq!(app.notice.as_deref(), Some("Recorded 2 reviews."));
}

/// A range picked with Shift and the arrows adds to what was already
/// picked, so that several ranges can follow one another. Going back within
/// a range lets go of its rows alone.
#[test]
fn a_new_range_adds_to_the_rows_already_picked() {
	let repo = Repo::new(
		json!({"docs/a.md": {}, "docs/b.md": {}, "docs/c.md": {}, "docs/d.md": {}, "docs/e.md": {}, "docs/f.md": {}}),
	);
	let (workspace, args) = (repo.workspace(), repo.args(&[]));
	let mut app = App::new(&workspace, &args).unwrap();
	assert_eq!(app.open.len(), 6);
	press(&mut app, &[Key::Extend(true), Key::Extend(true)]);
	assert_eq!((app.picked.len(), app.cursor), (3, Cursor::Open(2)));

	press(&mut app, &[Key::Down, Key::Down, Key::Extend(true)]);
	assert_eq!((app.picked.len(), app.cursor), (5, Cursor::Open(5)));
	assert!(
		!app.picked.contains(&app.open[3]),
		"the fourth was passed over"
	);
	press(&mut app, &[Key::Extend(false)]);
	assert_eq!((app.picked.len(), app.cursor), (4, Cursor::Open(4)));
	assert!(app.picked.contains(&app.open[0]), "the first range stays");

	press(
		&mut app,
		&[Key::Escape, Key::Char(' '), Key::Up, Key::Up, Key::Up],
	);
	press(&mut app, &[Key::Extend(false)]);
	assert_eq!((app.picked.len(), app.cursor), (3, Cursor::Open(0)));
	assert!(
		app.picked.contains(&app.open[4]),
		"the space bar's row stays"
	);
}

/// With the mouse, a drag picks the range of rows it passes over and an
/// ordinary click lets go of them all. A click made with Control or Shift
/// held is left alone, so that it keeps the meaning a terminal gives it.
#[test]
fn a_drag_picks_a_range_and_modified_clicks_are_left_alone() {
	use ratatui::crossterm::event::{
		KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
	};

	let repo = fixture();
	let (workspace, args) = (repo.workspace(), repo.args(&[]));
	let mut app = App::new(&workspace, &args).unwrap();
	show(&mut app, 120, 30);
	let at = |kind, modifiers| MouseEvent {
		kind,
		column: 10,
		row: 5,
		modifiers,
	};
	let down = MouseEventKind::Down(MouseButton::Left);
	let second = Target::Row(Cursor::Open(1));
	assert!(matches!(
		super::pointed(at(down, KeyModifiers::NONE), &app, 120),
		Some(super::Input::Click(target)) if target == second
	));
	assert!(super::pointed(at(down, KeyModifiers::CONTROL), &app, 120).is_none());
	assert!(super::pointed(at(down, KeyModifiers::SHIFT), &app, 120).is_none());
	let drag = MouseEventKind::Drag(MouseButton::Left);
	assert!(matches!(
		super::pointed(at(drag, KeyModifiers::NONE), &app, 120),
		Some(super::Input::Drag(target)) if target == second
	));

	app.drag(second);
	assert_eq!((app.picked.len(), app.cursor), (2, Cursor::Open(1)));
	app.click(Target::Row(Cursor::Open(0))).unwrap();
	assert!(app.picked.is_empty());
}

/// A plain yes needs something to review, so it leaves out a picked
/// document that needs nothing; a reread takes it.
#[test]
fn a_picked_document_that_needs_nothing_takes_a_reread_alone() {
	let repo = fixture();
	let (workspace, args) = (repo.workspace(), repo.args(&[]));
	let mut app = App::new(&workspace, &args).unwrap();
	press(&mut app, &[Key::Char('o'), Key::Down, Key::Down, Key::Down]);
	press(&mut app, &[Key::Char(' ')]);
	assert_eq!((app.picked.len(), app.cursor), (1, Cursor::Other(0)));
	press(&mut app, &[Key::Char('y')]);
	assert!(app.overlay.is_none());
	assert!(app.notice.as_ref().unwrap().contains("r records a reread"));
	press(&mut app, &[Key::Char('r')]);
	assert!(matches!(app.overlay, Some(Overlay::Picked(_, _, true))));
}

/// Picked sources limit a review to them, as `attest --source` does, and
/// the document stays in the queue for its other sources.
#[test]
fn picked_sources_limit_a_review_to_them() {
	let repo = Repo::new(
		json!({"docs/a.md": {"sources": [{"path": "/src/a.rs"}, {"path": "/src/b.rs"}]}}),
	);
	let (workspace, args) = (repo.workspace(), repo.args(&[]));
	let mut app = App::new(&workspace, &args).unwrap();
	press(&mut app, &[Key::Right, Key::Down, Key::Char(' ')]);
	assert_eq!((app.focus, app.file), (Focus::Sources, 1));
	assert_eq!(app.picked_sources.len(), 1);
	assert!(shows(
		&drawn(&app, 210, 44),
		"still accurate for the 1 picked source?"
	));

	press(&mut app, &[Key::Char('y'), Key::Enter]);
	let entry = repo.entry("docs/a.md");
	assert!(entry["sources"][0]["reviewed"].is_null());
	assert_eq!(entry["sources"][1]["reviewed"]["by"], "Test Reviewer");
	assert!(app.picked_sources.is_empty());
	assert_eq!(app.open.len(), 1, "its other source still waits");
	assert!(
		app
			.notice
			.as_ref()
			.unwrap()
			.contains("1 picked source alone")
	);
	assert_eq!(app.shown.as_ref().unwrap().files.len(), 1);
}

/// The document's name takes the focus above its sources, or in their
/// place when it has none, and Enter or a click shows it whole.
#[test]
fn the_name_of_the_document_is_shown_whole_on_request() {
	let repo = fixture();
	let (workspace, args) = (repo.workspace(), repo.args(&[]));
	let mut app = App::new(&workspace, &args).unwrap();
	press(&mut app, &[Key::Right, Key::Up]);
	assert_eq!(app.focus, Focus::Title);
	press(&mut app, &[Key::Enter]);
	let Some(Overlay::Name(name)) = &app.overlay else {
		panic!("expected the whole name");
	};
	assert!(name.ends_with("a.md"));
	press(&mut app, &[Key::Escape, Key::Down]);
	assert_eq!(app.focus, Focus::Sources);

	press(&mut app, &[Key::Left, Key::Down, Key::Right]);
	assert_eq!((app.cursor, app.focus), (Cursor::Open(1), Focus::Title));
	press(&mut app, &[Key::Escape]);
	assert_eq!(app.focus, Focus::Sidebar);

	show(&mut app, 120, 30);
	let title = app
		.hits
		.iter()
		.find(|(_, target)| *target == Target::Title)
		.map(|(area, _)| *area)
		.unwrap();
	let target = app.target_at(title.x, title.y).unwrap();
	app.click(target).unwrap();
	assert!(matches!(app.overlay, Some(Overlay::Name(_))));
}

/// A recorded review keeps what the screen read of the documents it did
/// not touch, so that they are not read again.
#[test]
fn what_was_read_survives_a_review_of_another_document() {
	let repo = fixture();
	let (workspace, args) = (repo.workspace(), repo.args(&[]));
	let mut app = App::new(&workspace, &args).unwrap();
	while app.prefetch() {}
	let other = app.open[1].clone();
	let before = Rc::clone(&app.cache[&other]);
	press(&mut app, &[Key::Char('y'), Key::Enter]);
	assert_eq!(app.open, std::slice::from_ref(&other));
	assert!(Rc::ptr_eq(&before, &app.cache[&other]));
	assert!(Rc::ptr_eq(&before, app.shown.as_ref().unwrap()));
}

/// Where the mark has no room, the smaller one takes its place with the
/// same glow, and where that has none either, nothing is drawn.
#[test]
fn the_mark_gives_way_to_the_smaller_one_and_then_to_nothing() {
	let repo = fixture();
	let (workspace, args) = (repo.workspace(), repo.args(&[]));
	let app = App::new(&workspace, &args).unwrap();
	let (large, small) = ("▄▄██████▄▄", "▄▄▀▀▀▀▄▄");
	let tall = drawn(&app, 120, 30);
	assert!(shows(&tall, large) && !shows(&tall, small));
	let short = drawn(&app, 120, 24);
	assert!(shows(&short, small) && !shows(&short, large));
	assert!(shows(&short, "▒"), "the smaller mark glows too");
	let shorter = drawn(&app, 120, 20);
	assert!(!shows(&shorter, small) && !shows(&shorter, large));
}

/// The files of a pattern are picked together, because a pattern is
/// reviewed as a whole, and a notice says so when picking one file brings
/// another along.
#[test]
fn the_files_of_a_pattern_are_picked_together_and_a_notice_says_so() {
	let repo =
		Repo::new(json!({"docs/a.md": {"sources": [{"pattern": "/src/*.rs"}]}}));
	let (workspace, args) = (repo.workspace(), repo.args(&[]));
	let mut app = App::new(&workspace, &args).unwrap();
	assert_eq!(app.shown.as_ref().unwrap().files.len(), 2);
	// Before any is picked, a bracket in the margin joins the two files.
	let margin = |app: &App| -> String {
		drawn(app, 120, 30)
			.iter()
			.filter_map(|row| row.chars().nth(32))
			.filter(|symbol| matches!(symbol, '┌' | '│' | '└' | '▌'))
			.collect()
	};
	assert_eq!(margin(&app), "┌└");
	press(&mut app, &[Key::Right, Key::Char(' ')]);
	assert_eq!(app.picked_sources.len(), 1);
	assert_eq!(margin(&app), "▌▌", "the bar takes the bracket's place");
	let notice = app.notice.clone().unwrap();
	assert!(notice.contains("`/src/*.rs` is a pattern"), "{notice}");
	assert!(
		notice.contains("its 2 changed files go together"),
		"{notice}"
	);
	// Both of its rows carry the bar, at the pane's left edge.
	let rows = drawn(&app, 120, 30);
	let bars = rows
		.iter()
		.filter(|row| row.chars().nth(32) == Some('▌'))
		.count();
	assert_eq!(bars, 2);

	// A range that already covers every file of the pattern needs no notice.
	press(&mut app, &[Key::Escape, Key::Extend(true)]);
	assert_eq!((app.picked_sources.len(), app.notice.as_ref()), (1, None));
}

/// On the label of the other documents nothing can be picked, so the space
/// bar opens and closes them as Enter does.
#[test]
fn the_space_bar_opens_the_other_documents_on_their_label() {
	let repo = fixture();
	let (workspace, args) = (repo.workspace(), repo.args(&[]));
	let mut app = App::new(&workspace, &args).unwrap();
	press(&mut app, &[Key::Down, Key::Down, Key::Char(' ')]);
	assert_eq!((app.cursor, app.expanded), (Cursor::Others, true));
	press(&mut app, &[Key::Char(' ')]);
	assert_eq!((app.expanded, app.picked.is_empty()), (false, true));
}

/// The bracket joins only neighbours of one source: its first file opens
/// it, its last closes it, and a file alone in its source has none.
#[test]
fn the_bracket_joins_the_files_of_one_source() {
	let owners = [0, 1, 1, 1, 2, 3, 3];
	let drawn: Vec<_> = (0..owners.len())
		.map(|index| draw::joint(&owners, index).unwrap_or(" "))
		.collect();
	assert_eq!(drawn.concat(), " ┌│└ ┌└");
}

/// Every action has a button in the last row, so that the mouse reaches it:
/// a click on one does what its key does. A notice sits in the rule above
/// the question and leaves the buttons in reach.
#[test]
fn the_buttons_of_the_last_row_take_clicks() {
	let repo = fixture();
	let (workspace, args) = (repo.workspace(), repo.args(&[]));
	let mut app = App::new(&workspace, &args).unwrap();
	show(&mut app, 120, 30);
	let button = |app: &App, key: Key| {
		app
			.hits
			.iter()
			.find(|(area, target)| area.y == 29 && *target == Target::Cap(key))
			.map(|(area, _)| *area)
	};
	for key in [
		Key::Char('?'),
		Key::Char('q'),
		Key::Char('v'),
		Key::Char('o'),
		Key::Char('s'),
		Key::Char('u'),
		Key::Char('h'),
		Key::Char(' '),
	] {
		assert!(button(&app, key).is_some(), "{key:?}");
	}
	let swap = button(&app, Key::Char('v')).unwrap();
	let target = app.target_at(swap.x + 1, swap.y).unwrap();
	app.click(target).unwrap();
	assert!(shows(&drawn(&app, 120, 30), "THE DOCUMENT"));
	app.click(Target::Cap(Key::Char('o'))).unwrap();
	assert!(app.expanded);
	app.click(Target::Cap(Key::Char(' '))).unwrap();
	assert_eq!(app.picked.len(), 1);
	app.click(Target::Cap(Key::Char('s'))).unwrap();
	assert!(matches!(app.overlay, Some(Overlay::Sources(_))));
	press(&mut app, &[Key::Escape]);

	app.notice = Some("A notice.".to_owned());
	let rows = drawn(&app, 120, 30);
	assert!(rows[27].contains(" A notice. "), "{}", rows[27]);
	assert!(rows[29].starts_with("  ?  keys"), "{}", rows[29]);
	assert_eq!(app.click(Target::Cap(Key::Char('q'))).unwrap(), Flow::Quit);
}

/// `s` lists every source of the document, changed or not: the files of a
/// pattern hang under it, each row says what became of its file, and Enter
/// on a changed one goes to it.
#[test]
fn every_source_is_listed_changed_or_not() {
	let repo = Repo::new(
		json!({"docs/a.md": {"sources": [{"path": "/src/a.rs"}, {"path": "/src/b.rs"}, {"pattern": "/lib/*.rs"}]}}),
	);
	repo.write("lib/one.rs", "one\n");
	repo.write("lib/two.rs", "two\n");
	repo.git(&["add", "--all"]);
	repo.git(&["commit", "--quiet", "-m", "Library"]);
	repo.attest("docs/a.md");
	repo.write("src/b.rs", "new b\n");
	repo.write("lib/two.rs", "two, changed\n");
	repo.write("lib/three.rs", "three\n");
	// A pattern matches the files git knows: the new one is staged.
	repo.git(&["add", "lib/three.rs"]);
	let (workspace, args) = (repo.workspace(), repo.args(&[]));
	let mut app = App::new(&workspace, &args).unwrap();
	assert!(shows(&drawn(&app, 120, 30), "3 changed of 5"));

	// Five files, and a row for the pattern that three of them match.
	press(&mut app, &[Key::Char('s')]);
	let listed = app.every_source();
	let row = |name: &str| {
		listed
			.iter()
			.find(|row| row.name == name)
			.unwrap_or_else(|| panic!("{name} in {listed:?}"))
	};
	assert_eq!(listed.len(), 6);
	assert_eq!(
		(row("src/a.rs").kept, row("src/a.rs").changed),
		(Some(Kept::Unchanged), None)
	);
	assert!(
		row("src/a.rs")
			.detail
			.starts_with("reviewed 2026-10-02 10:00 by Earlier Reviewer"),
		"{:?}",
		row("src/a.rs")
	);
	assert_eq!(row("src/b.rs").kept, Some(Kept::Changed));
	assert_eq!(row("src/b.rs").detail, "in the working tree");
	assert_eq!(
		(row("/lib/*.rs").kept, row("/lib/*.rs").joint),
		(None, None)
	);
	assert!(
		row("/lib/*.rs")
			.detail
			.starts_with("pattern · 3 files · reviewed"),
		"{:?}",
		row("/lib/*.rs")
	);
	assert_eq!(row("lib/one.rs").kept, Some(Kept::Unchanged));
	assert_eq!(row("lib/three.rs").kept, Some(Kept::New));
	assert_eq!(row("lib/two.rs").kept, Some(Kept::Changed));
	let joints: String = listed.iter().filter_map(|row| row.joint).collect();
	assert_eq!(joints, "├├└");

	// The box opens on the first changed file, and says what each row is.
	let Some(Overlay::Sources(at)) = app.overlay else {
		panic!("expected the list of sources");
	};
	assert_eq!(listed[at].changed, Some(0));
	let rows = drawn(&app, 120, 30);
	for text in [
		"5 files of docs/a.md: 3 changed",
		"unchanged",
		"changed +1 -1",
		"new +1 -0",
		"in the working tree",
	] {
		assert!(shows(&rows, text), "{text}:\n{}", rows.join("\n"));
	}

	// The cursor moves over every row; a click on a row moves it there, and
	// a second one acts as Enter does.
	press(&mut app, &[Key::Up; 6]);
	assert_eq!(app.overlay, Some(Overlay::Sources(0)));
	press(&mut app, &[Key::Enter]);
	assert_eq!(app.overlay, Some(Overlay::Sources(0)), "nothing to go to");
	press(&mut app, &[Key::Wheel(true, Area::Pane), Key::PageDown]);
	assert_eq!(app.overlay, Some(Overlay::Sources(5)));
	press(&mut app, &[Key::PageUp]);
	let last = listed
		.iter()
		.rposition(|row| row.changed.is_some())
		.unwrap();
	show(&mut app, 120, 30);
	assert!(
		app
			.hits
			.iter()
			.any(|(_, target)| *target == Target::Listed(last))
	);
	app.click(Target::Listed(last)).unwrap();
	assert_eq!(app.overlay, Some(Overlay::Sources(last)));
	app.click(Target::Listed(last)).unwrap();
	assert_eq!(
		(app.overlay.is_none(), app.focus, app.file),
		(true, Focus::Sources, 2)
	);

	// A document without a source has nothing to list.
	let repo = fixture();
	let (workspace, args) = (repo.workspace(), repo.args(&[]));
	let mut app = App::new(&workspace, &args).unwrap();
	press(&mut app, &[Key::Down, Key::Char('s')]);
	assert_eq!(
		(app.overlay.is_none(), app.notice.as_deref()),
		(true, Some("This document has no source to list."))
	);
}

/// `u` lists the answers of the session, and `u` there takes the newest
/// back: the lockfile holds again what it held, byte for byte, and the
/// document returns to the queue without an invented reason.
#[test]
fn the_newest_answer_is_taken_back() {
	let repo = fixture();
	let lockfile = repo.read("source-sentry.lock");
	let (workspace, args) = (repo.workspace(), repo.args(&[]));
	let mut app = App::new(&workspace, &args).unwrap();
	press(&mut app, &[Key::Char('u')]);
	assert!(app.overlay.is_none());
	assert!(app.notice.as_ref().unwrap().starts_with("Nothing recorded"));

	press(&mut app, &[Key::Char('y'), Key::Enter]);
	press(&mut app, &[Key::Char('y'), Key::Enter]);
	assert_eq!((app.open.len(), app.session.recorded.len()), (0, 2));
	assert_ne!(repo.read("source-sentry.lock"), lockfile);
	press(&mut app, &[Key::Char('u')]);
	assert_eq!(app.overlay, Some(Overlay::Log(None)));
	let rows = drawn(&app, 120, 30);
	assert!(shows(&rows, "2 answers recorded in this session."));
	assert!(shows(&rows, "docs/a.md") && shows(&rows, "docs/b.md"));

	// While another answer stands the box stays, and says what was done.
	press(&mut app, &[Key::Char('u')]);
	assert_eq!(
		app.overlay,
		Some(Overlay::Log(Some(
			"Took back the review of docs/b.md.".to_owned()
		)))
	);
	assert!(repo.entry("docs/b.md")["reviewed"].is_null());
	assert!(repo.entry("docs/a.md")["sources"][0]["reviewed"].is_object());
	press(&mut app, &[Key::Char('u')]);
	assert_eq!(repo.read("source-sentry.lock"), lockfile);
	assert_eq!(
		(app.open.len(), app.session.recorded.len(), app.progress()),
		(2, 0, (0, 2))
	);
	assert!(app.overlay.is_none());
	assert_eq!(
		app.notice.as_deref(),
		Some("Took back the review of docs/a.md.")
	);
	let shown = app.shown.as_ref().unwrap();
	assert_eq!((app.cursor, shown.reason.as_ref()), (Cursor::Open(0), None));
	assert!(shown.name.ends_with("a.md"));
	assert!(
		app
			.closing()
			.plain()
			.contains("2 documents still need review")
	);
}

/// One answer given to several documents is taken back whole. An answer is
/// not taken back once a file it wrote has changed again.
#[test]
fn a_shared_answer_is_taken_back_whole_and_a_changed_file_refuses() {
	let repo = fixture();
	let lockfile = repo.read("source-sentry.lock");
	let (workspace, args) = (repo.workspace(), repo.args(&[]));
	let mut app = App::new(&workspace, &args).unwrap();
	press(&mut app, &[Key::Char('a'), Key::Char('y'), Key::Char('u')]);
	assert_eq!(app.log().len(), 1);
	assert!(
		app.log()[0]
			.1
			.starts_with("2 documents: docs/a.md, docs/b.md"),
		"{:?}",
		app.log()
	);
	press(&mut app, &[Key::Char('u')]);
	assert_eq!(app.notice.as_deref(), Some("Took back 2 reviews."));
	assert_eq!(repo.read("source-sentry.lock"), lockfile);
	assert_eq!((app.open.len(), app.session.recorded.len()), (2, 0));

	// Another review of the same lockfile, made outside the session, stands
	// in the way: the answer is kept, and the refusal names the file.
	press(&mut app, &[Key::Char('y'), Key::Enter]);
	repo.attest("docs/b.md");
	press(&mut app, &[Key::Char('u'), Key::Char('u')]);
	let notice = app.notice.clone().unwrap();
	assert!(
		notice.starts_with("source-sentry.lock is no longer as that answer left"),
		"{notice}"
	);
	assert_eq!(
		(app.overlay.is_none(), app.session.recorded.len()),
		(true, 1)
	);
	assert!(repo.entry("docs/a.md")["sources"][0]["reviewed"].is_object());
}

/// `h` lists the commits that changed the selected source since its review,
/// newest first, and says when the working tree changes it further.
#[test]
fn the_commits_that_changed_a_source_are_listed() {
	let repo = Repo::new(
		json!({"docs/a.md": {"sources": [{"path": "/src/a.rs"}, {"path": "/src/b.rs"}]}}),
	);
	repo.attest("docs/a.md");
	let commit = |subject: &str| {
		repo.git(&["add", "--all"]);
		repo.git(&["commit", "--quiet", "-m", subject]);
	};
	commit("Record the review");
	repo.write("src/a.rs", "second a\n");
	commit("Change a once");
	repo.write("src/a.rs", "third a\n");
	commit("Change a twice");
	repo.write("src/a.rs", "fourth a\n");
	repo.write("src/b.rs", "new b\n");
	let (workspace, args) = (repo.workspace(), repo.args(&[]));
	let mut app = App::new(&workspace, &args).unwrap();

	press(&mut app, &[Key::Char('h')]);
	let Some(Overlay::History(past, 0)) = &app.overlay else {
		panic!("expected the history, not {:?}", app.overlay);
	};
	assert_eq!(past.path, "src/a.rs");
	let subjects: Vec<_> = past
		.commits
		.iter()
		.map(|commit| commit.subject.as_str())
		.collect();
	assert_eq!(subjects, ["Change a twice", "Change a once"]);
	assert_eq!((past.uncommitted, past.more, past.rows()), (true, false, 3));
	assert_eq!(
		past.head,
		"The commits that changed it since its review of 2026-10-02 10:00."
	);
	let rows = drawn(&app, 120, 30);
	for text in [
		"Not committed: changes in the working tree",
		"2026-10-01  Test Reviewer",
		"Change a twice",
	] {
		assert!(shows(&rows, text), "{text}:\n{}", rows.join("\n"));
	}
	// The wheel moves the list and keeps the box.
	press(&mut app, &[Key::Wheel(true, Area::Pane)]);
	assert!(matches!(app.overlay, Some(Overlay::History(_, 2))));

	// A source changed in the working tree alone has no commit to list.
	press(
		&mut app,
		&[Key::Escape, Key::Right, Key::Down, Key::Char('h')],
	);
	let Some(Overlay::History(past, 0)) = &app.overlay else {
		panic!("expected the history, not {:?}", app.overlay);
	};
	assert_eq!((past.path.as_str(), past.commits.len()), ("src/b.rs", 0));
	assert_eq!(past.head, "No commit has changed it since its review.");
	assert!(past.uncommitted);

	// An answer for one source alone says so in the session's list.
	press(
		&mut app,
		&[Key::Escape, Key::Char(' '), Key::Char('y'), Key::Enter],
	);
	assert!(app.log()[0].1.ends_with("docs/a.md · 1 source alone"));
}
