//! `diff`: each source's changes since its attestation, and the `attest`
//! command that binds exactly what was shown.

use std::fmt::Write as _;

use similar::{ChangeTag, TextDiff};
use source_sentry_core::mutate::{
	Baseline, Current, DocumentDiff, EntryDiff, Selection, diff,
};
use source_sentry_core::ports::Snapshot as _;
use source_sentry_core::report::{DiffReport, to_json};

use crate::cli::{DiffArgs, Format};
use crate::describe;
use crate::exit::{self, Failure};
use crate::output::{self, Printer};
use crate::workspace::{Workspace, load};

/// Lines of context around each change.
const CONTEXT_LINES: usize = 3;

/// Runs `diff` on the working tree.
///
/// # Errors
///
/// Exit 2, 3 or 4 when `diff` refuses or the repository cannot be read, and
/// 64 for a document that cannot be named.
pub(super) fn run(
	workspace: &Workspace,
	args: &DiffArgs,
	printer: Printer,
) -> Result<u8, Failure> {
	let snapshot = workspace
		.repository
		.working_tree_snapshot()
		.map_err(|error| Failure::backend(&error))?;
	let (_, inventory) = load(&snapshot)?;
	let document = workspace.document(&args.document)?;
	let selection = if args.sources.is_empty() {
		Selection::All
	} else {
		Selection::Sources(args.sources.clone())
	};
	let compared = diff(
		&snapshot,
		&workspace.repository,
		&inventory,
		&document,
		&selection,
	)
	.map_err(|error| Failure::mutate(&error))?;
	let attest = attest_command(workspace, args, &compared);
	match args.format {
		Format::Json => {
			let report =
				DiffReport::new(&compared, snapshot.revision(), attest, plain_hunks);
			let json = to_json(&report).map_err(|error| {
				Failure::new(
					exit::OPERATIONAL,
					format!("cannot write the diff: {error}"),
				)
			})?;
			output::data(&json);
		}
		Format::Human => printer.print(&render(workspace, &compared, &attest)),
	}
	Ok(exit::SUCCESS)
}

/// The arguments of the `attest` command that binds what `diff` showed:
/// the same document and selection, and every expectation.
fn attest_command(
	workspace: &Workspace,
	args: &DiffArgs,
	compared: &DocumentDiff,
) -> Vec<String> {
	let mut command = vec![source_sentry_core::SLUG.to_owned()];
	if let Some(repo) = workspace.repo_argument() {
		command.push("--repo".to_owned());
		command.push(repo.display().to_string());
	}
	command.push("attest".to_owned());
	command.push(workspace.display_document(&compared.document));
	for source in &args.sources {
		command.push("--source".to_owned());
		command.push(source.clone());
	}
	for expectation in &compared.expectations {
		command.push("--expect".to_owned());
		command.push(expectation.to_string());
	}
	command
}

/// The human output: each source with its files and their diffs, then the
/// `attest` command on its own line, last.
fn render(
	workspace: &Workspace,
	compared: &DocumentDiff,
	attest: &[String],
) -> String {
	let changed = compared
		.sources
		.iter()
		.flat_map(|source| &source.entries)
		.filter(|entry| entry.is_changed())
		.count();
	let mut text = format!(
		"{} · {} changed since attested\n",
		output::bold(&workspace.display_document(&compared.document)),
		describe::count(changed, "file", "files")
	);
	for source in &compared.sources {
		let kind = if source.pattern { " (pattern)" } else { "" };
		let _ = write!(text, "\n{}{kind}\n", output::bold(&source.declared));
		if source.entries.is_empty() {
			text.push_str("  matches no file\n");
		}
		for entry in &source.entries {
			entry_text(&mut text, entry);
		}
	}
	let line = attest
		.iter()
		.map(|word| shell_word(word))
		.collect::<Vec<_>>()
		.join(" ");
	let _ = write!(
		text,
		"\nAfter the review, record exactly what was shown with:\n  {line}"
	);
	text
}

/// One file: what changed, and its diff.
fn entry_text(text: &mut String, entry: &EntryDiff) {
	let attested = match &entry.baseline {
		Baseline::Unattested => None,
		Baseline::Unavailable { oid } | Baseline::Available { oid, .. } => {
			Some(describe::short(oid).to_owned())
		}
	};
	let current = match &entry.current {
		Current::Missing => None,
		Current::Present { oid, .. } => Some(describe::short(oid).to_owned()),
	};
	let status = match (&attested, &current) {
		_ if !entry.is_changed() => format!(
			"unchanged since attested at {}",
			attested.as_deref().unwrap_or_default()
		),
		(Some(attested), Some(current)) => {
			format!("changed: {attested} → {current}")
		}
		(None, Some(current)) => format!("never attested; now {current}"),
		(Some(attested), None) => {
			format!("no longer matches its pattern; attested at {attested}")
		}
		(None, None) => "absent".to_owned(),
	};
	let _ = writeln!(text, "  {}  {status}", output::bold(entry.path.as_str()));
	if !entry.is_changed() {
		return;
	}
	let old = match &entry.baseline {
		Baseline::Unattested => Some(""),
		Baseline::Unavailable { .. } => {
			text.push_str(
				"  baseline unavailable: the attested version is not in the object \
				 database, so review the whole file\n",
			);
			return;
		}
		Baseline::Available { content, .. } => std::str::from_utf8(content).ok(),
	};
	let new = match &entry.current {
		Current::Missing => Some(""),
		Current::Present { content, .. } => std::str::from_utf8(content).ok(),
	};
	let (Some(old), Some(new)) = (old, new) else {
		text.push_str("  binary content is not shown\n");
		return;
	};
	let path = entry.path.as_str();
	let _ = writeln!(
		text,
		"  {}\n  {}",
		output::removed(&format!(
			"--- {}",
			attested.map_or_else(
				|| "/dev/null".to_owned(),
				|oid| format!("{path} ({oid})")
			)
		)),
		output::added(&format!(
			"+++ {}",
			current.map_or_else(
				|| "/dev/null".to_owned(),
				|oid| format!("{path} ({oid})")
			)
		))
	);
	coloured_hunks(text, old, new);
}

/// The hunks of a unified diff, coloured, indented by two spaces.
fn coloured_hunks(text: &mut String, old: &str, new: &str) {
	let diff = TextDiff::from_lines(old, new);
	for hunk in diff
		.unified_diff()
		.context_radius(CONTEXT_LINES)
		.iter_hunks()
	{
		let _ = writeln!(text, "  {}", output::hunk(&hunk.header().to_string()));
		for change in hunk.iter_changes() {
			let value = change.value();
			let line = value
				.strip_suffix('\n')
				.map_or(value, |line| line.strip_suffix('\r').unwrap_or(line));
			let styled = match change.tag() {
				ChangeTag::Delete => output::removed(&format!("-{line}")),
				ChangeTag::Insert => output::added(&format!("+{line}")),
				ChangeTag::Equal => format!(" {line}"),
			};
			let _ = writeln!(text, "  {styled}");
			if change.missing_newline() {
				text.push_str("  \\ No newline at end of file\n");
			}
		}
	}
}

/// The hunks of a unified diff, without file headers or colour, for JSON.
fn plain_hunks(old: &str, new: &str) -> String {
	let diff = TextDiff::from_lines(old, new);
	let mut unified = diff.unified_diff();
	unified.context_radius(CONTEXT_LINES);
	unified.iter_hunks().map(|hunk| hunk.to_string()).collect()
}

/// `word` as one shell word: bare when it is made only of characters that
/// no common shell treats specially, and single-quoted otherwise.
fn shell_word(word: &str) -> String {
	let bare = !word.is_empty()
		&& word.chars().all(|character| {
			character.is_ascii_alphanumeric() || "-_./:@%+=,".contains(character)
		});
	if bare {
		word.to_owned()
	} else {
		format!("'{}'", word.replace('\'', r"'\''"))
	}
}

#[cfg(test)]
mod tests {
	use super::{plain_hunks, shell_word};

	#[test]
	fn words_are_quoted_only_when_needed() {
		assert_eq!(
			shell_word("db/schema.sql@sha1:9c1185a5"),
			"db/schema.sql@sha1:9c1185a5"
		);
		assert_eq!(shell_word("docs/a b.md"), "'docs/a b.md'");
		assert_eq!(shell_word("it's"), r"'it'\''s'");
		assert_eq!(shell_word("/db/*.sql"), "'/db/*.sql'");
	}

	#[test]
	fn plain_hunks_have_no_file_headers() {
		assert_eq!(
			plain_hunks("a\nb\n", "a\nc\n"),
			"@@ -1,2 +1,2 @@\n a\n-b\n+c\n"
		);
	}
}
