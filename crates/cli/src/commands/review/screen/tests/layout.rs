//! How the panes lay text out: wrapping by the cells a character takes,
//! scrolling through every wrapped row, and headings that stay within their
//! pane at any size.

use std::rc::Rc;

use super::super::app::{App, Area, Key, Pane};
use super::super::draw;
use super::{Repo, drawn, fixture, json, press, shows};

/// Forty-eight different ideographs, each two cells wide.
fn ideographs() -> String {
	('\u{4e00}'..).take(48).collect()
}

/// A row keeps to its cells: a wide character that does not fit moves whole
/// to the next row, a combining mark stays with the letter before it, and
/// every character lands in exactly one row.
#[test]
fn rows_wrap_by_the_cells_each_character_takes() {
	let wide = ideographs();
	let rows = draw::wrap(&wide, 9);
	assert!(rows.iter().all(|row| row.chars().count() == 4), "{rows:?}");
	assert_eq!(rows.concat(), wide);
	let combined = "e\u{301}e\u{301}e\u{301}";
	assert_eq!(draw::wrap(combined, 2), ["e\u{301}e\u{301}", "e\u{301}"]);
	assert_eq!(draw::wrap("ab\u{200b}cd", 2), ["ab\u{200b}", "cd"]);
	assert_eq!(draw::wrap("", 5), [""]);
}

/// A line of wide characters shows every one of them, in the changes and in
/// the document.
#[test]
fn wide_characters_wrap_whole_in_both_panes() {
	let repo = fixture();
	let line = ideographs();
	repo.write("docs/a.md", &format!("{line}\n"));
	repo.write("src/a.rs", &format!("{line}\n"));
	let (workspace, args) = (repo.workspace(), repo.args(&[]));
	let mut app = App::new(&workspace, &args).unwrap();
	for pane in ["changes", "document"] {
		let screen = drawn(&app, 80, 20).join("\n");
		let missing: String = line
			.chars()
			.filter(|wide| !screen.contains(*wide))
			.collect();
		assert!(missing.is_empty(), "the {pane} lose {missing}:\n{screen}");
		press(&mut app, &[Key::Char('v')]);
	}
}

/// A line longer than the pane is tall scrolls row by row to its end: a page
/// at a time, by the wheel and back, in the changes and in the document. A
/// wider screen wraps it in fewer rows, and the position keeps within them.
#[test]
fn a_line_taller_than_the_pane_scrolls_to_its_last_row() {
	let repo = fixture();
	let long = format!("{}WRAPPED_TAIL", "x".repeat(1200));
	repo.write(
		"src/a.rs",
		&format!(
			"{long}
"
		),
	);
	repo.write(
		"docs/a.md",
		&format!(
			"{long}
"
		),
	);
	let (workspace, args) = (repo.workspace(), repo.args(&[]));
	let mut app = App::new(&workspace, &args).unwrap();
	app.page = 6;
	assert!(!shows(&drawn(&app, 80, 20), "WRAPPED_TAIL"));
	// At 80 columns, a change's text takes 38 cells of each row, so the line
	// alone takes 32 rows.
	let last = draw::pane_rows(&app, Pane::Changes, 80) - 1;
	assert!(last >= 32, "{last}");
	press(&mut app, &[Key::PageDown]);
	assert_eq!(app.scroll.0, 6);
	press(&mut app, &[Key::PageDown; 10]);
	assert_eq!(app.scroll.0, last);
	assert!(shows(&drawn(&app, 80, 20), "WRAPPED_TAIL"));
	press(&mut app, &[Key::Wheel(false, Area::Pane)]);
	assert_eq!(app.scroll.0, last - 3);
	// At 120 columns the line takes 16 rows; the position drawn and the next
	// page start from the last of them.
	press(&mut app, &[Key::PageDown; 10]);
	let wider = draw::pane_rows(&app, Pane::Changes, 120) - 1;
	assert!(wider < last, "{wider}");
	assert!(shows(&drawn(&app, 120, 30), "WRAPPED_TAIL"));
	press(&mut app, &[Key::PageUp]);
	assert_eq!(app.scroll.0, wider - 6);
	// The document's text takes 40 cells of each row at 80 columns: 31 rows.
	press(&mut app, &[Key::Char('v')]);
	assert!(!shows(&drawn(&app, 80, 20), "WRAPPED_TAIL"));
	press(&mut app, &[Key::PageDown; 10]);
	assert_eq!(app.scroll.1, 30);
	assert!(shows(&drawn(&app, 80, 20), "WRAPPED_TAIL"));
}

/// The cells of `row` from column `x`, as drawn.
fn from_column(row: &str, x: usize) -> String {
	row.chars().skip(x).collect()
}

/// The pane's heading keeps within the pane: facts too long for it are cut
/// with an ellipsis, after the outcome, and the sidebar beside it keeps its
/// divider and its count of documents.
#[test]
fn the_pane_heading_keeps_within_the_pane() {
	let repo = fixture();
	let (workspace, args) = (repo.workspace(), repo.args(&[]));
	let mut app = App::new(&workspace, &args).unwrap();
	let screen = drawn(&app, 80, 20);
	assert_eq!(screen[2].chars().nth(28), Some('2'), "{}", screen[2]);
	assert_eq!(screen[2].chars().nth(30), Some('│'), "{}", screen[2]);
	let heading = from_column(&screen[2], 31);
	assert!(heading.starts_with(" warn  overdue"), "{heading}");
	assert!(heading.ends_with('…'), "{heading}");
	// Facts as wide as the screen are cut in the same way.
	Rc::make_mut(app.shown.as_mut().unwrap()).facts = "x".repeat(79);
	let screen = drawn(&app, 80, 20);
	assert_eq!(screen[2].chars().nth(30), Some('│'), "{}", screen[2]);
	let heading = from_column(&screen[2], 31);
	assert_eq!(heading, format!(" warn  {}…", "x".repeat(40)));
}

/// The whole screen draws at the smallest size it supports and just below
/// it, with long facts, a long path, no changes to show and a document of
/// wide characters, in both panes. At the smallest size the sidebar keeps
/// its divider on every row.
#[test]
fn the_screen_draws_at_its_smallest_size_and_below() {
	let path = format!("docs/{}/{}.md", "nested".repeat(12), "name".repeat(10));
	let repo = Repo::new(json!({
		path.as_str(): {"sources": [{"path": "/src/a.rs"}]},
	}));
	repo.write(&path, &format!("{}\n\n{}\n", ideographs(), ideographs()));
	let (workspace, args) = (repo.workspace(), repo.args(&[]));
	let mut app = App::new(&workspace, &args).unwrap();
	let shown = Rc::make_mut(app.shown.as_mut().unwrap());
	shown.facts = format!("{}, {}", "y".repeat(60), "z".repeat(60));
	shown.files.clear();
	let (columns, rows) = draw::MINIMUM;
	for _ in ["changes", "document"] {
		for (width, height) in [
			(columns, rows),
			(columns - 1, rows),
			(columns, rows - 1),
			(columns - 1, rows - 1),
		] {
			let screen = drawn(&app, width, height);
			if (width, height) != draw::MINIMUM {
				continue;
			}
			for row in &screen[2..usize::from(rows) - 3] {
				assert_eq!(row.chars().nth(30), Some('│'), "{}", screen.join("\n"));
			}
		}
		press(&mut app, &[Key::Char('v')]);
	}
}
