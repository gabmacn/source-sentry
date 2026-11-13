//! The line mode's display of a document: a ruled header, then each pending
//! file with its changes, under a line budget shared by the whole document.

use std::fmt::Write as _;

use source_sentry_core::mutate::DocumentDiff;

use super::View;
use super::changes::{self, Entry, Row};
use crate::workspace::Workspace;
use crate::{describe, output};

/// The maximum number of diff lines in a document's initial display.
const PREVIEW_LINES: usize = 40;

/// Whether text carries terminal styles, or stays plain as a file needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Paint {
	/// Styled for a terminal; the output stream removes styles it cannot use.
	Styled,
	/// Plain text, for the full diff's file and for comparing in tests.
	Plain,
}

impl Paint {
	/// `text` through `style` when styled, and unchanged when plain.
	fn with(self, text: &str, style: fn(&str) -> String) -> String {
		match self {
			Self::Styled => style(text),
			Self::Plain => text.to_owned(),
		}
	}
}

/// The document's place, name, facts and bounded comparison, before its
/// question. `width` is the measure for the rule and the right-hand column.
pub(super) fn document(
	workspace: &Workspace,
	view: &View,
	paint: Paint,
	width: usize,
) -> String {
	let position = format!("{} of {}", view.position, view.total);
	let rule = "─".repeat(width.saturating_sub(position.chars().count() + 4));
	let mut text = format!(
		"\n{} {} {}\n",
		paint.with("──", output::dim),
		paint.with(&position, output::bold),
		paint.with(&rule, output::dim)
	);
	let name = workspace.display_document(&view.compared.document);
	let outcome = output::outcome_name(view.outcome);
	let label = match paint {
		Paint::Styled => output::outcome_label(view.outcome),
		Paint::Plain => outcome.to_owned(),
	};
	text.push_str(&spread(
		&format!("  {}", paint.with(&name, output::bold)),
		2 + name.chars().count(),
		&format!("{label}  {}", view.facts),
		outcome.len() + 2 + view.facts.chars().count(),
		width,
	));
	text.push('\n');
	if !view.project.is_root() {
		let _ =
			writeln!(text, "  Project {}", workspace.display_path(&view.project));
	}
	if let Some(reason) = &view.reason {
		let _ = writeln!(text, "  {reason}");
	}
	text.push('\n');
	text.push_str(&changes(
		workspace,
		&view.compared,
		Some(PREVIEW_LINES),
		paint,
		width,
	));
	text
}

/// Every pending file with its object names, line counts and located
/// changes. `limit` bounds the diff lines across the document; each file's
/// own line is always shown.
pub(super) fn changes(
	workspace: &Workspace,
	compared: &DocumentDiff,
	limit: Option<usize>,
	paint: Paint,
	width: usize,
) -> String {
	let mut text = String::new();
	let mut left = limit.unwrap_or(usize::MAX);
	let mut omitted = 0;
	let mut pending = false;
	for source in &compared.sources {
		if source.entries.is_empty() && source.reviewed.is_none() {
			text.push_str("  A source pattern matches no file and was never attested; correct it before recording a review.\n");
		}
		for entry in source.entries.iter().filter(|entry| entry.is_changed()) {
			if pending {
				text.push('\n');
			}
			pending = true;
			file(
				&changes::entry(workspace, entry),
				&mut text,
				&mut left,
				&mut omitted,
				paint,
				width,
			);
		}
	}
	if compared.sources.is_empty() {
		text.push_str(
			"  No sources: `y` and `r` both record a whole-document reread.\n",
		);
	} else if !pending {
		text.push_str(
			"  No source content changed; the facts above still require review.\n",
		);
	}
	if omitted > 0 {
		let note = format!(
			"{} · d opens the full diff",
			describe::count(omitted, "more diff line", "more diff lines")
		);
		let _ = writeln!(text, "\n  {}", paint.with(&note, output::dim));
	}
	text
}

/// One file: its path with the counts and object names at the right, then
/// its rows behind a bar. A location line opens each group of changes.
fn file(
	entry: &Entry,
	text: &mut String,
	left: &mut usize,
	omitted: &mut usize,
	paint: Paint,
	width: usize,
) {
	let names = format!("{} → {}", entry.old, entry.new);
	let (counts, counts_width) = entry.counts.map_or_else(
		|| (String::new(), 0),
		|(added, removed)| {
			let (added, removed) = (format!("+{added}"), format!("-{removed}"));
			(
				format!(
					"{} {}   ",
					paint.with(&added, output::added),
					paint.with(&removed, output::removed)
				),
				added.len() + removed.len() + 4,
			)
		},
	);
	text.push_str(&spread(
		&format!("  {}", entry.path),
		2 + entry.path.chars().count(),
		&format!("{counts}{}", paint.with(&names, output::dim)),
		counts_width + names.chars().count(),
		width,
	));
	text.push('\n');
	let bar = paint.with("│", output::dim);
	let mut shown = false;
	for row in &entry.rows {
		let line = match row {
			Row::Hunk(line) if *left > 0 => {
				paint.with(&format!("{}:{line}", entry.path), output::hunk)
			}
			Row::Line {
				number,
				sign,
				text: value,
			} => {
				shown = *left > 0;
				if !shown {
					*omitted += 1;
					continue;
				}
				*left -= 1;
				let number = format!("{number:>4}");
				match sign {
					'+' => paint.with(&format!("{number} + {value}"), output::added),
					'-' => paint.with(&format!("{number} - {value}"), output::removed),
					_ => format!("{}   {value}", paint.with(&number, output::dim)),
				}
			}
			Row::NoNewline if shown => {
				"       \\ No newline at end of file".to_owned()
			}
			Row::Note(note) => (*note).to_owned(),
			Row::Hunk(_) | Row::NoNewline => continue,
		};
		let _ = writeln!(text, "  {bar} {}", line.trim_end());
	}
}

/// `left` and `right` on one line, pushed apart to `width` columns. At least
/// two spaces stay between them when the line is too long for that.
fn spread(
	left: &str,
	left_width: usize,
	right: &str,
	right_width: usize,
	width: usize,
) -> String {
	let gap = width.saturating_sub(left_width + right_width).max(2);
	format!("{left}{}{right}", " ".repeat(gap))
}
