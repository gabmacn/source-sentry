//! A pending file's changes as rows, shared by every way of showing them.

use similar::{ChangeTag, TextDiff};
use source_sentry_core::model::ObjectId;
use source_sentry_core::mutate::{Baseline, Current, EntryDiff};

use crate::workspace::Workspace;

/// The digits of an object name shown to people; the lockfile holds them all.
const SHORT_OID: usize = 7;
/// The columns between tab stops when a line is shown.
const TAB_STOP: usize = 4;

/// One row of a file's changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Row {
	/// The line at which a group of changes starts, for jumping to it.
	Hunk(usize),
	/// One line of the comparison.
	Line {
		/// Its number: in the old file when removed, in the new otherwise.
		number: usize,
		/// `+` when added, `-` when removed, a space when unchanged.
		sign: char,
		/// Its text, with tabs expanded and no line ending.
		text: String,
	},
	/// The line above this row has no newline at the end of its file.
	NoNewline,
	/// A statement standing in for lines that cannot be shown.
	Note(&'static str),
}

/// One pending file: what it was, what it is, and what changed between.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Entry {
	/// The path, relative to the current directory.
	pub(super) path: String,
	/// The attested object name, shortened, or `unattested`.
	pub(super) old: String,
	/// The current object name, shortened, or `absent`.
	pub(super) new: String,
	/// Lines added and removed, when both versions can be read as text.
	pub(super) counts: Option<(usize, usize)>,
	/// The changes, in file order.
	pub(super) rows: Vec<Row>,
}

/// Compares one file with its attested version.
pub(super) fn entry(workspace: &Workspace, entry: &EntryDiff) -> Entry {
	let mut shown = Entry {
		path: workspace.display_path(&entry.path),
		old: match &entry.baseline {
			Baseline::Unattested => "unattested".to_owned(),
			Baseline::Unavailable { oid } | Baseline::Available { oid, .. } => {
				short(oid)
			}
		},
		new: match &entry.current {
			Current::Missing => "absent".to_owned(),
			Current::Present { oid, .. } => short(oid),
		},
		counts: None,
		rows: Vec::new(),
	};
	let old = match &entry.baseline {
		Baseline::Unattested => Some(""),
		Baseline::Unavailable { .. } => {
			shown.rows.push(Row::Note(
				"Baseline unavailable; the whole current file follows.",
			));
			Some("")
		}
		Baseline::Available { content, .. } => std::str::from_utf8(content).ok(),
	};
	let new = match &entry.current {
		Current::Missing => Some(""),
		Current::Present { content: None, .. } => {
			shown.rows.push(Row::Note("Lines added and removed unavailable: canonical content cannot be rebuilt; stage this file with git to show its changes."));
			return shown;
		}
		Current::Present {
			content: Some(content),
			..
		} => std::str::from_utf8(content).ok(),
	};
	let (Some(old), Some(new)) = (old, new) else {
		shown.rows.push(Row::Note(
			"Lines added and removed unavailable: binary content is not shown.",
		));
		return shown;
	};
	let diff = TextDiff::from_lines(old, new);
	let (mut added, mut removed) = (0, 0);
	for group in diff.grouped_ops(3) {
		let mut first = true;
		for op in group {
			for change in diff.iter_changes(&op) {
				let (sign, index) = match change.tag() {
					ChangeTag::Insert => {
						added += 1;
						('+', change.new_index())
					}
					ChangeTag::Delete => {
						removed += 1;
						('-', change.old_index())
					}
					ChangeTag::Equal => (' ', change.new_index()),
				};
				let number = index.map_or(1, |index| index + 1);
				if first {
					shown.rows.push(Row::Hunk(number));
					first = false;
				}
				shown.rows.push(Row::Line {
					number,
					sign,
					text: expand_tabs(change.value().trim_end_matches(['\n', '\r'])),
				});
				if change.missing_newline() {
					shown.rows.push(Row::NoNewline);
				}
			}
		}
	}
	shown.counts = Some((added, removed));
	shown
}

/// The first digits of an object name, without its algorithm.
fn short(oid: &ObjectId) -> String {
	oid.hex().chars().take(SHORT_OID).collect()
}

/// `text` with each tab replaced by spaces up to the next tab stop, so that
/// indentation survives whatever is printed before the line.
pub(super) fn expand_tabs(text: &str) -> String {
	let mut expanded = String::with_capacity(text.len());
	let mut column = 0;
	for character in text.chars() {
		if character == '\t' {
			let spaces = TAB_STOP - column % TAB_STOP;
			expanded.extend(std::iter::repeat_n(' ', spaces));
			column += spaces;
		} else {
			expanded.push(character);
			column += 1;
		}
	}
	expanded
}

#[cfg(test)]
mod tests {
	use super::expand_tabs;

	/// Tabs reach the next stop from wherever they start.
	#[test]
	fn tabs_expand_to_the_next_stop() {
		assert_eq!(expand_tabs("\tlet a;"), "    let a;");
		assert_eq!(expand_tabs("ab\tc"), "ab  c");
		assert_eq!(expand_tabs("\t\tx"), "        x");
		assert_eq!(expand_tabs("plain"), "plain");
	}
}
