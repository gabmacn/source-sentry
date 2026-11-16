//! How the panes lay text out: wrapping by the cells a character takes,
//! scrolling through every wrapped row, and headings that stay within their
//! pane at any size.

use super::super::app::{App, Key};
use super::super::draw;
use super::{drawn, fixture, press};

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
