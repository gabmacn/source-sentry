//! Located source changes, with a shared line budget for the prompt.

use std::fmt::Write as _;

use similar::{ChangeTag, TextDiff};
use source_sentry_core::mutate::{Baseline, Current, DocumentDiff, EntryDiff};

use super::View;
use crate::workspace::Workspace;

/// The maximum number of diff lines in a document's initial display.
const PREVIEW_LINES: usize = 40;

/// The document's place, facts and bounded comparison, before its question.
pub(super) fn document(workspace: &Workspace, view: &View) -> String {
	let mut text = format!(
		"\n[{}/{}] {}: {}\n",
		view.position,
		view.total,
		workspace.display_document(&view.compared.document),
		view.facts
	);
	if let Some(reason) = &view.reason {
		let _ = writeln!(text, "{reason}");
	}
	text.push_str(&changes(workspace, &view.compared, Some(PREVIEW_LINES)));
	text
}

/// Every pending source's oids and line counts, with located hunks. `limit`
/// bounds diff lines across the document; summaries are always shown.
pub(super) fn changes(
	workspace: &Workspace,
	compared: &DocumentDiff,
	limit: Option<usize>,
) -> String {
	let mut text = String::new();
	let mut left = limit.unwrap_or(usize::MAX);
	let mut omitted = 0;
	let mut pending = false;
	for source in &compared.sources {
		if source.entries.is_empty() {
			text.push_str("A source pattern matches no file and was never attested; correct it before recording a review.\n");
		}
		for entry in source.entries.iter().filter(|entry| entry.is_changed()) {
			pending = true;
			entry_text(workspace, entry, &mut text, &mut left, &mut omitted);
		}
	}
	if compared.sources.is_empty() {
		text.push_str(
			"No sources: `y` and `r` both record a whole-document reread.\n",
		);
	} else if !pending {
		text.push_str(
			"No source content changed; the facts above still require review.\n",
		);
	}
	if omitted > 0 {
		let _ = writeln!(
			text,
			"{omitted} diff line(s) omitted; `d` opens the full diff."
		);
	}
	text
}

/// Renders a pending file, retaining metadata even when the line budget is
/// exhausted. Removed lines use their old position; other lines use the new.
fn entry_text(
	workspace: &Workspace,
	entry: &EntryDiff,
	text: &mut String,
	left: &mut usize,
	omitted: &mut usize,
) {
	let path = workspace.display_path(&entry.path);
	let old_oid = match &entry.baseline {
		Baseline::Unattested => "unattested".to_owned(),
		Baseline::Unavailable { oid } | Baseline::Available { oid, .. } => {
			oid.to_string()
		}
	};
	let new_oid = match &entry.current {
		Current::Missing => "absent".to_owned(),
		Current::Present { oid, .. } => oid.to_string(),
	};
	let _ = writeln!(text, "\n{path}:1  {old_oid} -> {new_oid}");
	let old = match &entry.baseline {
		Baseline::Unattested => Some(""),
		Baseline::Unavailable { .. } => {
			text
				.push_str("  Baseline unavailable; the whole current file follows.\n");
			Some("")
		}
		Baseline::Available { content, .. } => std::str::from_utf8(content).ok(),
	};
	let new = match &entry.current {
		Current::Missing => Some(""),
		Current::Present { content: None, .. } => {
			text.push_str("  Lines added and removed unavailable: canonical content cannot be rebuilt; stage this file with git to show its changes.\n");
			return;
		}
		Current::Present {
			content: Some(content),
			..
		} => std::str::from_utf8(content).ok(),
	};
	let (Some(old), Some(new)) = (old, new) else {
		text.push_str(
			"  Lines added and removed unavailable: binary content is not shown.\n",
		);
		return;
	};
	let diff = TextDiff::from_lines(old, new);
	let mut added = 0;
	let mut removed = 0;
	for change in diff.iter_all_changes() {
		match change.tag() {
			ChangeTag::Insert => added += 1,
			ChangeTag::Delete => removed += 1,
			ChangeTag::Equal => {}
		}
	}
	let _ = writeln!(text, "  +{added} -{removed} lines");
	for group in diff.grouped_ops(3) {
		for op in group {
			for change in diff.iter_changes(&op) {
				if *left == 0 {
					*omitted += 1;
					continue;
				}
				*left -= 1;
				let (sign, index) = match change.tag() {
					ChangeTag::Insert => ('+', change.new_index()),
					ChangeTag::Delete => ('-', change.old_index()),
					ChangeTag::Equal => (' ', change.new_index()),
				};
				let line = index.map_or(1, |index| index + 1);
				let value = change.value().trim_end_matches(['\n', '\r']);
				let _ = writeln!(text, "{path}:{line}  {sign}{value}");
				if change.missing_newline() {
					text.push_str("  \\ No newline at end of file\n");
				}
			}
		}
	}
}
