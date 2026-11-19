//! The boxes that can hold more than they show: the confirmation before
//! several documents are recorded, a whole path or name, the projects, and
//! the answers of the session. Each scrolls inside itself, so that
//! everything it covers can be read.

use std::rc::Rc;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::super::app::{App, Area, Cursor, Field, Key, Overlay, Target};
use super::{Repo, fixture, json, press, show};

/// The keys and turns of the wheel that scroll a box and never close it.
const SCROLLING: [Key; 6] = [
	Key::Down,
	Key::PageDown,
	Key::Up,
	Key::PageUp,
	Key::Wheel(true, Area::Pane),
	Key::Wheel(false, Area::Sidebar),
];

/// The screen as drawn, with the open box's place.
struct Drawn {
	/// The screen's rows of text.
	rows: Vec<String>,
	/// The box, border included.
	frame: Rect,
	/// The grid itself.
	buffer: Buffer,
}

impl Drawn {
	/// The text of the box's rows, between its side borders.
	fn inside(&self) -> Vec<String> {
		(self.frame.y..self.frame.bottom())
			.map(|y| {
				(self.frame.x + 1..self.frame.right() - 1)
					.map(|x| self.buffer[(x, y)].symbol())
					.collect()
			})
			.collect()
	}

	/// Whether the box draws a scrollbar in its rightmost inner column.
	fn scrollbar(&self) -> bool {
		let column = self.frame.right() - 2;
		(self.frame.y + 1..self.frame.bottom() - 1)
			.any(|y| self.buffer[(column, y)].symbol() == "▐")
	}

	/// The box's lower border, whole.
	fn lower_border(&self) -> String {
		let y = self.frame.bottom() - 1;
		(self.frame.x..self.frame.right())
			.map(|x| self.buffer[(x, y)].symbol())
			.collect()
	}

	/// The screen's rows below the box.
	fn below(&self) -> &[String] {
		&self.rows[usize::from(self.frame.bottom())..]
	}
}

/// Draws the screen with a box open, as the terminal loop does.
fn boxed(app: &mut App, columns: u16, rows: u16) -> Drawn {
	let buffer = show(app, columns, rows);
	let frame = app
		.hits
		.iter()
		.find(|(_, target)| *target == Target::Box)
		.map(|(area, _)| *area)
		.expect("a box is open");
	let text = (0..rows)
		.map(|y| {
			(0..columns)
				.map(|x| buffer[(x, y)].symbol())
				.collect::<String>()
		})
		.collect();
	Drawn {
		rows: text,
		frame,
		buffer,
	}
}

/// Scrolls the open box down a row at a time from its top to its end,
/// drawing it at each position, and returns each drawing. The box must stay
/// open throughout.
fn scroll_through(app: &mut App, columns: u16, rows: u16) -> Vec<Drawn> {
	press(app, &[Key::PageUp; 40]);
	let mut drawn = vec![boxed(app, columns, rows)];
	loop {
		let at = app.box_scroll;
		press(app, &[Key::Down]);
		assert!(app.overlay.is_some(), "Down closed the box");
		if app.box_scroll == at {
			return drawn;
		}
		drawn.push(boxed(app, columns, rows));
	}
}

/// A whole value of some 1,400 characters, which ends with a tail to look
/// for.
fn long_value() -> String {
	format!("{}WHOLE_VALUE_TAIL", "deeply/nested/".repeat(100))
}

/// A whole name or path longer than its box can show scrolls inside the box
/// to its last characters. At no position does it draw over the box's
/// lower border or the rows below the box; on a tall screen it fits, with
/// no scrollbar.
#[test]
fn a_long_whole_value_scrolls_to_its_end_within_its_box() {
	let repo = fixture();
	let (workspace, args) = (repo.workspace(), repo.args(&[]));
	let mut app = App::new(&workspace, &args).unwrap();
	let value = long_value();
	Rc::make_mut(app.shown.as_mut().unwrap())
		.name
		.clone_from(&value);
	press(&mut app, &[Key::Right, Key::Up, Key::Enter]);
	assert_eq!(app.overlay, Some(Overlay::Name(value.clone())));
	for kind in ["name", "path"] {
		if kind == "path" {
			press(&mut app, &[Key::Escape]);
			Rc::make_mut(app.shown.as_mut().unwrap()).files[0]
				.path
				.clone_from(&value);
			press(&mut app, &[Key::Down, Key::Enter]);
			assert_eq!(app.overlay, Some(Overlay::Path(value.clone())));
		}
		let positions = scroll_through(&mut app, 80, 20);
		assert!(positions.len() > 1, "the {kind} scrolls");
		let first = &positions[0];
		assert!(
			!first.inside().concat().contains("TAIL"),
			"{:#?}",
			first.rows
		);
		let last = positions.last().unwrap();
		assert!(
			last.inside().concat().contains("WHOLE_VALUE_TAIL"),
			"the {kind} shows its end:\n{:#?}",
			last.rows
		);
		for drawn in &positions {
			assert!(drawn.scrollbar());
			let border = drawn.lower_border();
			assert!(
				border.starts_with('└')
					&& border.ends_with('┘')
					&& border
						.chars()
						.skip(1)
						.take(border.chars().count() - 2)
						.all(|c| c == '─'),
				"{border}"
			);
			assert_eq!(drawn.below(), first.below());
			for row in drawn.below() {
				assert!(!row.contains("nested") && !row.contains("TAIL"), "{row}");
			}
			// Every row of the box keeps within its side borders.
			for y in drawn.frame.y..drawn.frame.bottom() {
				let row = &drawn.rows[usize::from(y)];
				let right: String =
					row.chars().skip(usize::from(drawn.frame.right())).collect();
				assert!(!right.contains("nested"), "{row}");
			}
		}
		// The keys and the wheel keep the box open and come back to its top.
		press(&mut app, &SCROLLING);
		press(&mut app, &[Key::PageUp; 10]);
		assert_eq!((app.box_scroll, app.overlay.is_some()), (0, true));

		let tall = boxed(&mut app, 120, 40);
		assert!(tall.inside().concat().contains("WHOLE_VALUE_TAIL"));
		assert!(!tall.scrollbar(), "it fits on a tall screen");
		press(&mut app, &[Key::Down, Key::PageDown]);
		assert_eq!((app.box_scroll, app.overlay.is_some()), (0, true));
	}
	press(&mut app, &[Key::Escape]);
	assert!(app.overlay.is_none(), "Escape closes it");
	press(&mut app, &[Key::Enter]);
	assert!(matches!(app.overlay, Some(Overlay::Path(_))));
	press(&mut app, &[Key::Enter]);
	assert!(app.overlay.is_none(), "so does any other key");
}

/// A whole value that fits draws no scrollbar, and the keys and the wheel
/// that would scroll it leave it as it is, open.
#[test]
fn a_whole_value_that_fits_neither_scrolls_nor_closes_on_the_arrows() {
	let repo = fixture();
	let (workspace, args) = (repo.workspace(), repo.args(&[]));
	let mut app = App::new(&workspace, &args).unwrap();
	press(&mut app, &[Key::Right, Key::Enter]);
	assert!(matches!(app.overlay, Some(Overlay::Path(_))));
	let before = boxed(&mut app, 80, 20);
	assert!(!before.scrollbar());
	for key in SCROLLING {
		press(&mut app, &[key]);
		assert!(app.overlay.is_some(), "{key:?} closed it");
		assert_eq!(boxed(&mut app, 80, 20).rows, before.rows, "{key:?}");
	}
	press(&mut app, &[Key::Escape]);
	assert!(app.overlay.is_none());
}

/// Thirteen projects: the root and twelve nested ones. The box lists them
/// all; on the smallest screen the thirteenth is reached by scrolling, and
/// on a tall one every project fits, with no scrollbar.
#[test]
fn every_project_is_listed_and_the_thirteenth_is_reached_by_scrolling() {
	let repo = Repo::new(json!({"docs/a.md": {}}));
	for index in 1..=12 {
		repo.write(
			&format!("project{index:02}/.source-sentry/source-sentry.yaml"),
			"version: 1
",
		);
	}
	repo.git(&["add", "--all"]);
	repo.git(&["commit", "--quiet", "-m", "Twelve nested projects"]);
	let (workspace, args) = (repo.workspace(), repo.args(&[]));
	let mut app = App::new(&workspace, &args).unwrap();
	assert_eq!(app.session.state.projects.len(), 13);
	app.click(Target::Field(Field::Projects)).unwrap();
	assert_eq!(app.overlay, Some(Overlay::Projects));

	let positions = scroll_through(&mut app, 80, 20);
	assert!(!positions[0].inside().concat().contains("project12"));
	assert!(positions[0].scrollbar());
	let last = positions.last().unwrap();
	assert!(
		last.inside().concat().contains("project12"),
		"{:#?}",
		last.rows
	);
	for index in 1..=12 {
		let project = format!("project{index:02}");
		assert!(
			positions
				.iter()
				.any(|drawn| drawn.inside().concat().contains(&project)),
			"{project} is never shown"
		);
	}
	press(&mut app, &SCROLLING);
	assert_eq!(app.overlay, Some(Overlay::Projects));

	let tall = boxed(&mut app, 120, 40);
	assert!(tall.inside().concat().contains("project12"));
	assert!(tall.inside().concat().contains("project01"));
	assert!(!tall.scrollbar());

	press(&mut app, &[Key::Escape]);
	assert!(app.overlay.is_none(), "Escape closes it");
}

/// A list of projects that fits draws no scrollbar, and the keys and the
/// wheel that would scroll it leave it open. Escape closes it, and Down
/// then returns from the header to the documents, as before.
#[test]
fn a_list_of_projects_that_fits_neither_scrolls_nor_closes_on_the_arrows() {
	let repo = fixture();
	let (workspace, args) = (repo.workspace(), repo.args(&[]));
	let mut app = App::new(&workspace, &args).unwrap();
	press(&mut app, &[Key::Up, Key::Right, Key::Right, Key::Enter]);
	assert_eq!(app.overlay, Some(Overlay::Projects));
	let before = boxed(&mut app, 80, 20);
	assert!(!before.scrollbar());
	for key in SCROLLING {
		press(&mut app, &[key]);
		assert_eq!(app.overlay, Some(Overlay::Projects), "{key:?} closed it");
		assert_eq!(boxed(&mut app, 80, 20).rows, before.rows, "{key:?}");
	}
	press(&mut app, &[Key::Escape, Key::Down]);
	assert_eq!((app.overlay.is_none(), app.cursor), (true, Cursor::Open(0)));
}

/// A repository with `count` documents that need review, `docs/d00.md`
/// onwards, which is the order of the queue.
fn many(count: usize) -> Repo {
	let entries: serde_json::Map<String, serde_json::Value> = (0..count)
		.map(|index| (format!("docs/d{index:02}.md"), json!({})))
		.collect();
	Repo::new(entries.into())
}

/// The documents a drawing of the confirmation lists, in its rows' order.
fn listed(drawn: &Drawn) -> Vec<String> {
	drawn
		.inside()
		.iter()
		.filter_map(|row| {
			let at = row.find("docs/d")?;
			row.get(at..at + 11).map(str::to_owned)
		})
		.collect()
}

/// Scrolls a confirmation from its top to its end at a size, and checks
/// what every position shows: the question and the keys, a window of the
/// documents in the order they will be recorded, and over the whole scroll
/// every document exactly once, the last included.
fn confirmation_lists_each_once(
	app: &mut App,
	(columns, rows): (u16, u16),
	(ask, key): (&str, &str),
	count: usize,
) {
	let order: Vec<String> = (0..count)
		.map(|index| format!("docs/d{index:02}.md"))
		.collect();
	let positions = scroll_through(app, columns, rows);
	let mut seen: Vec<String> = Vec::new();
	for drawn in &positions {
		let inside = drawn.inside().join("\n");
		assert!(inside.contains(ask), "the question stays:\n{inside}");
		assert!(inside.contains(key), "the keys stay:\n{inside}");
		let window = listed(drawn);
		assert!(window.len() >= 3, "{inside}");
		let start = order.iter().position(|name| *name == window[0]).unwrap();
		assert_eq!(window, order[start..start + window.len()], "{inside}");
		for name in window {
			if !seen.contains(&name) {
				seen.push(name);
			}
		}
	}
	assert_eq!(seen, order, "every document, each once, in order");
	let last = positions.last().unwrap();
	assert_eq!(listed(last).last(), order.last());
	assert_eq!(last.scrollbar(), positions.len() > 1);
}

/// `a` lists every document it covers, eleven or twenty-five, scrolled
/// inside its box on the smallest screen and on a comfortable one. The
/// question and its keys stay in sight at every position, and `y` records
/// every document, the last ones included.
#[test]
fn the_confirmation_of_a_lists_every_document_and_records_them_all() {
	for count in [11, 25] {
		let repo = many(count);
		let (workspace, args) = (repo.workspace(), repo.args(&[]));
		let mut app = App::new(&workspace, &args).unwrap();
		press(&mut app, &[Key::Char('a')]);
		assert!(
			matches!(&app.overlay, Some(Overlay::All(batch)) if batch.due.len() == count)
		);
		let ask = format!("Record the {count} documents that are due");
		for size in [(80, 20), (120, 30)] {
			confirmation_lists_each_once(&mut app, size, (&ask, "n  go back"), count);
		}
		// Ten rows on the smallest screen hold neither list; twenty-three on
		// the comfortable one hold the shorter.
		press(&mut app, &[Key::PageUp; 10]);
		assert!(boxed(&mut app, 80, 20).scrollbar());
		assert_eq!(boxed(&mut app, 120, 30).scrollbar(), count > 23);
		press(&mut app, &SCROLLING);
		press(&mut app, &[Key::PageDown; 10]);
		assert!(matches!(app.overlay, Some(Overlay::All(_))));
		press(&mut app, &[Key::Char('y')]);
		assert!(app.overlay.is_none());
		assert_eq!(app.session.recorded.len(), count);
		let last = format!("docs/d{:02}.md", count - 1);
		assert!(repo.entry(&last)["reviewed"].is_object());
	}
}

/// The keys that leave the confirmation of `a` do as before: `n`, Escape
/// and Enter go back without recording; the arrows, the page keys and the
/// wheel only scroll.
#[test]
fn the_confirmation_of_a_still_goes_back_on_its_keys() {
	let repo = many(11);
	let (workspace, args) = (repo.workspace(), repo.args(&[]));
	let mut app = App::new(&workspace, &args).unwrap();
	for leave in [Key::Char('n'), Key::Escape, Key::Enter] {
		press(&mut app, &[Key::Char('a')]);
		boxed(&mut app, 80, 20);
		for key in SCROLLING {
			press(&mut app, &[key]);
			assert!(
				matches!(app.overlay, Some(Overlay::All(_))),
				"{key:?} closed it"
			);
		}
		press(&mut app, &[leave]);
		assert!(app.overlay.is_none(), "{leave:?} goes back");
		assert_eq!(app.session.recorded, Vec::new());
	}
}

/// Picked documents are confirmed in the same box: it lists all of them,
/// scrolls on the same keys, and `y` records each one.
#[test]
fn the_confirmation_of_picked_documents_lists_and_records_them_all() {
	let repo = many(11);
	let (workspace, args) = (repo.workspace(), repo.args(&[]));
	let mut app = App::new(&workspace, &args).unwrap();
	press(&mut app, &[Key::Extend(true); 10]);
	assert_eq!(app.picked.len(), 11);
	press(&mut app, &[Key::Char('y')]);
	assert!(matches!(app.overlay, Some(Overlay::Picked(..))));
	confirmation_lists_each_once(
		&mut app,
		(80, 20),
		("Record the 11 picked documents", "n  go back"),
		11,
	);
	press(&mut app, &[Key::Escape]);
	assert!(app.overlay.is_none());
	assert_eq!(app.session.recorded, Vec::new());
	press(&mut app, &[Key::Char('y')]);
	press(&mut app, &SCROLLING);
	press(&mut app, &[Key::Char('y')]);
	assert_eq!(app.session.recorded.len(), 11);
	assert!(repo.entry("docs/d10.md")["reviewed"].is_object());
}

/// A confirmation that fits draws no scrollbar, and the keys and the wheel
/// that would scroll it leave it open and as it was.
#[test]
fn a_confirmation_that_fits_neither_scrolls_nor_closes_on_the_arrows() {
	let repo = fixture();
	let (workspace, args) = (repo.workspace(), repo.args(&[]));
	let mut app = App::new(&workspace, &args).unwrap();
	press(&mut app, &[Key::Char('a')]);
	let before = boxed(&mut app, 80, 20);
	assert!(!before.scrollbar());
	for key in SCROLLING {
		press(&mut app, &[key]);
		assert!(matches!(app.overlay, Some(Overlay::All(_))), "{key:?}");
		assert_eq!(boxed(&mut app, 80, 20).rows, before.rows, "{key:?}");
	}
}

/// The box of `u` lists every answer of the session that stands, twelve or
/// twenty-five, newest first, scrolled inside it where they do not all fit:
/// the smallest screen holds eleven, and a tall one both lists.
/// Its head and its keys stay in sight. The arrows, the page keys and the
/// wheel scroll it and never close it; `u` still takes the newest back from
/// anywhere in the list, which then returns to its top.
#[test]
fn the_log_lists_every_answer_that_stands_and_scrolls_inside_its_box() {
	for count in [12, 25] {
		let repo = many(count);
		let (workspace, args) = (repo.workspace(), repo.args(&[]));
		let mut app = App::new(&workspace, &args).unwrap();
		for _ in 0..count {
			press(&mut app, &[Key::Char('y'), Key::Enter]);
		}
		assert_eq!(app.log().len(), count);
		press(&mut app, &[Key::Char('u')]);
		assert_eq!(app.overlay, Some(Overlay::Log(None)));
		let newest_first: Vec<String> = (0..count)
			.rev()
			.map(|index| format!("docs/d{index:02}.md"))
			.collect();
		let head = format!("{count} answers recorded in this session.");
		for (columns, rows) in [(80, 20), (120, 40)] {
			let positions = scroll_through(&mut app, columns, rows);
			assert_eq!(positions.len() > 1, rows == 20, "{columns} by {rows}");
			let mut seen: Vec<String> = Vec::new();
			for drawn in &positions {
				let inside = drawn.inside().join(
					"
",
				);
				assert!(
					inside.contains(&head),
					"the head stays:
{inside}"
				);
				assert!(
					inside.contains("u  take back"),
					"the keys stay:
{inside}"
				);
				assert_eq!(drawn.scrollbar(), positions.len() > 1);
				let window = listed(drawn);
				assert!(window.len() >= 3, "{inside}");
				let start = newest_first
					.iter()
					.position(|name| *name == window[0])
					.unwrap();
				assert_eq!(
					window,
					newest_first[start..start + window.len()],
					"{inside}"
				);
				for name in window {
					if !seen.contains(&name) {
						seen.push(name);
					}
				}
			}
			assert_eq!(seen, newest_first, "every answer, each once, in order");
		}

		press(&mut app, &[Key::PageUp; 10]);
		boxed(&mut app, 80, 20);
		press(&mut app, &SCROLLING);
		press(&mut app, &[Key::PageDown; 10]);
		assert!(app.box_scroll > 0);
		assert_eq!(app.overlay, Some(Overlay::Log(None)));
		press(&mut app, &[Key::Char('u')]);
		let newest = format!("docs/d{:02}.md", count - 1);
		assert!(repo.entry(&newest)["reviewed"].is_null());
		assert!(repo.entry("docs/d00.md")["reviewed"].is_object());
		assert_eq!(app.log().len(), count - 1);
		assert!(matches!(app.overlay, Some(Overlay::Log(Some(_)))));
		assert_eq!(app.box_scroll, 0);
		press(&mut app, &[Key::Escape]);
		assert!(app.overlay.is_none(), "Escape closes it");
	}
}

/// The box of contexts is not one that scrolls inside itself, and keeps
/// its keys: the arrows move its cursor, the wheel does nothing, and the
/// page keys and Escape close it.
#[test]
fn the_context_choices_keep_their_keys() {
	let repo = fixture();
	repo.write(
		".source-sentry/source-sentry.yaml",
		"version: 1\ndefine:\n  contexts:\n    production:\n      fallback: warn\n    staging:\n      fallback: warn\n    testing:\n      fallback: warn\n",
	);
	repo.git(&["add", "--all"]);
	repo.git(&["commit", "--quiet", "-m", "Contexts"]);
	let (workspace, args) = (repo.workspace(), repo.args(&[]));
	let mut app = App::new(&workspace, &args).unwrap();
	assert!(app.contexts().len() >= 3, "{:?}", app.contexts());
	let open = |app: &mut App| {
		app.click(Target::Field(Field::Context)).unwrap();
		assert!(matches!(app.overlay, Some(Overlay::Contexts(_))));
	};
	open(&mut app);
	let Some(Overlay::Contexts(at)) = app.overlay else {
		panic!("expected the contexts");
	};
	press(&mut app, &[Key::Down]);
	assert_eq!(app.overlay, Some(Overlay::Contexts(at + 1)));
	press(&mut app, &[Key::Wheel(true, Area::Pane)]);
	assert_eq!(app.overlay, Some(Overlay::Contexts(at + 1)));
	press(&mut app, &[Key::Up]);
	assert_eq!(app.overlay, Some(Overlay::Contexts(at)));
	for leave in [Key::PageDown, Key::PageUp, Key::Escape] {
		press(&mut app, &[leave]);
		assert!(app.overlay.is_none(), "{leave:?} closes it");
		open(&mut app);
	}
}
