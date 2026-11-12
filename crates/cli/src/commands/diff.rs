//! `diff`: each source's changes since its attestation, and the `attest`
//! command that binds exactly what was shown.

use std::fmt::Write as _;

use similar::{ChangeTag, TextDiff};
use source_sentry_core::claims::Inventory;
use source_sentry_core::mutate::{
	Baseline, Current, DocumentDiff, EntryDiff, Selection, SourceSpec, diff,
};
use source_sentry_core::ports::Snapshot as _;
use source_sentry_core::report::{DiffReport, to_json};

use crate::cli::{DiffArgs, DocumentArg, Format, parse_document};
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
	let (_, inventory) = load(workspace, &snapshot)?;
	let document = workspace.document(&args.document)?;
	let selection = workspace.selection(&inventory, &document, &args.sources)?;
	let compared = diff(
		&snapshot,
		&workspace.repository,
		&inventory,
		&document,
		&selection,
	)
	.map_err(|error| Failure::mutate(workspace, &error))?;
	let attest = attest_command(workspace, args, &inventory, &compared)?;
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
/// the same document and selection, and every expectation. A document with
/// no sources can only be reviewed by rereading it, so the command then
/// carries `--reread`.
fn attest_command(
	workspace: &Workspace,
	args: &DiffArgs,
	inventory: &Inventory,
	compared: &DocumentDiff,
) -> Result<Vec<String>, Failure> {
	let mut command = vec![source_sentry_core::SLUG.to_owned()];
	if let (Format::Human, Some(repo)) = (args.format, workspace.repo_argument())
	{
		option(&mut command, "--repo", repo.display().to_string());
	}
	command.push("attest".to_owned());
	let document = match (args.format, &compared.document) {
		(Format::Json, source_sentry_core::model::DocumentId::Local(path)) => {
			path.as_str().to_owned()
		}
		_ => workspace.display_document(&compared.document),
	};
	// Keep a local path from reading as an option or a remote identity.
	let local_identity =
		matches!(
			compared.document,
			source_sentry_core::model::DocumentId::Local(_)
		) && matches!(parse_document(&document), Ok(DocumentArg::Remote(_)));
	command.push(if document.starts_with('-') || local_identity {
		format!("./{document}")
	} else {
		document
	});
	for source in &args.sources {
		let selector = command_selector(
			workspace,
			inventory,
			&compared.document,
			source,
			args.format,
		)?;
		option(&mut command, "--source", selector);
	}
	for expectation in &compared.expectations {
		let path = match args.format {
			Format::Human => workspace.display_path(expectation.path()),
			Format::Json => format!("/{}", expectation.path().as_str()),
		};
		let state = expectation.to_string();
		let state = state.rsplit_once('@').map_or("absent", |(_, state)| state);
		option(&mut command, "--expect", format!("{path}@{state}"));
	}
	if compared.sources.is_empty() {
		command.push("--reread".to_owned());
	}
	Ok(command)
}

/// A path selector relative to the current directory for people, or rooted
/// for JSON; pattern selectors keep their spelling.
/// Redundant `./` components distinguish a path from a pattern with the
/// same rooted spelling. A finite declaration has only finitely many such
/// spellings, so inserting them must find an unambiguous path selector.
fn command_selector(
	workspace: &Workspace,
	inventory: &Inventory,
	document: &source_sentry_core::model::DocumentId,
	selector: &str,
	format: Format,
) -> Result<String, Failure> {
	let selected =
		workspace.selection(inventory, document, &[selector.to_owned()])?;
	if !matches!(selected, Selection::Declared(sources) if sources.iter().any(|source| matches!(source, SourceSpec::Path(_))))
	{
		return Ok(selector.to_owned());
	}
	let path = workspace.source_path(std::path::Path::new(selector))?;
	let mut shown = match format {
		Format::Human => workspace.display_path(&path),
		Format::Json => format!("/{}", path.as_str()),
	};
	let attempts = inventory
		.claims
		.iter()
		.find(|claim| &claim.document == document)
		.map_or(0, |claim| claim.declaration.sources.len());
	for _ in 0..attempts {
		if workspace
			.selection(inventory, document, std::slice::from_ref(&shown))
			.is_ok()
		{
			return Ok(shown);
		}
		shown.insert_str(usize::from(shown.starts_with('/')), "./");
	}
	workspace.selection(inventory, document, std::slice::from_ref(&shown))?;
	Ok(shown)
}

/// Appends one option, protecting a value that begins with `-` from parsing
/// as another option when the command is copied.
fn option(command: &mut Vec<String>, flag: &str, value: String) {
	if value.starts_with('-') {
		command.push(format!("{flag}={value}"));
	} else {
		command.push(flag.to_owned());
		command.push(value);
	}
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
		let _ = write!(
			text,
			"\n{}{kind}\n",
			output::bold(&format!("`{}`", source.declared))
		);
		// No match and no attested member: `attest` refuses such a pattern.
		if source.entries.is_empty() {
			text.push_str(
				"  matches no file and was never attested, so `attest` refuses it: \
				 correct the pattern, or `untrack` it\n",
			);
		}
		for entry in &source.entries {
			entry_text(workspace, &mut text, entry);
		}
	}
	let line = attest
		.iter()
		.map(|word| shell_word(word))
		.collect::<Vec<_>>()
		.join(" ");
	if compared.sources.is_empty() {
		text.push_str(
			"\nThis document declares no sources, so reviewing it means rereading \
			 it.\n",
		);
	}
	let _ = write!(
		text,
		"\nAfter the review, record exactly what was shown with:\n  {line}"
	);
	text
}

/// One file: what changed, and its diff.
fn entry_text(workspace: &Workspace, text: &mut String, entry: &EntryDiff) {
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
	let _ = writeln!(
		text,
		"  {}  {status}",
		output::bold(&workspace.display_path(&entry.path))
	);
	if !entry.is_changed() {
		return;
	}
	let old = match &entry.baseline {
		Baseline::Unattested => Some(""),
		Baseline::Unavailable { .. } => {
			text.push_str(
				"  baseline unavailable: the attested version is not in the object \
				 database, so the whole current file follows; review all of it\n",
			);
			Some("")
		}
		Baseline::Available { content, .. } => std::str::from_utf8(content).ok(),
	};
	let new = match &entry.current {
		Current::Missing => Some(""),
		Current::Present { content: None, .. } => {
			text.push_str(
				"  content not shown: git stores this file converted, for example \
				 by `working-tree-encoding`; `git add` it to show its changes\n",
			);
			return;
		}
		Current::Present {
			content: Some(content),
			..
		} => std::str::from_utf8(content).ok(),
	};
	let (Some(old), Some(new)) = (old, new) else {
		text.push_str("  binary content is not shown\n");
		return;
	};
	let path = workspace.display_path(&entry.path);
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

/// `word` as one shell word that POSIX shells and PowerShell read alike:
///
/// - bare, when it is made only of characters that no common shell treats
///   specially, and does not begin with PowerShell's `@` expression marker;
/// - single-quoted, when it holds no single quotation mark. PowerShell also
///   reads the typographic marks `‘ ’ ‚ ‛` as one;
/// - double-quoted, when it holds one, but nothing that either shell expands
///   or ends a double-quoted word with: `$`, `` ` ``, `\`, `!`, or a double
///   quotation mark, typographic ones included;
/// - otherwise in the POSIX form `'it'\''s'`, which PowerShell cannot read.
pub(crate) fn shell_word(word: &str) -> String {
	const SINGLE: [char; 5] =
		['\'', '\u{2018}', '\u{2019}', '\u{201A}', '\u{201B}'];
	const IN_DOUBLE: [char; 8] =
		['$', '`', '\\', '!', '"', '\u{201C}', '\u{201D}', '\u{201E}'];
	let bare = !word.is_empty()
		&& !word.starts_with('@')
		&& word.chars().all(|character| {
			character.is_ascii_alphanumeric() || "-_./:@%+=".contains(character)
		});
	if bare {
		word.to_owned()
	} else if !word.contains(SINGLE) {
		format!("'{word}'")
	} else if !word.contains(IN_DOUBLE) {
		format!("\"{word}\"")
	} else {
		format!("'{}'", word.replace('\'', r"'\''"))
	}
}

#[cfg(test)]
mod tests {
	use super::{plain_hunks, shell_word};

	/// PowerShell treats leading at signs as expression or splatting syntax.
	#[test]
	fn leading_at_signs_are_quoted() {
		for word in ["@doc.md", "@source.rs@sha1:9c1185a5", "@@doc.md"] {
			assert_eq!(shell_word(word), format!("'{word}'"));
		}
	}

	/// Leading comments and redirections must remain literal file names.
	#[test]
	fn leading_comment_and_redirection_characters_are_quoted() {
		for word in ["#doc.md", "<doc.md", ">doc.md"] {
			assert_eq!(shell_word(word), format!("'{word}'"));
		}
		for word in ["+doc.md", "=doc.md", "%doc.md", ":doc.md"] {
			assert_eq!(shell_word(word), word);
		}
	}

	/// PowerShell rejects initial, consecutive and trailing unquoted commas.
	#[test]
	fn commas_are_quoted() {
		for word in [",doc.md", "doc,,name.md", "doc.md,"] {
			assert_eq!(shell_word(word), format!("'{word}'"));
		}
	}

	#[test]
	fn words_are_quoted_only_when_needed() {
		assert_eq!(
			shell_word("db/schema.sql@sha1:9c1185a5"),
			"db/schema.sql@sha1:9c1185a5"
		);
		assert_eq!(shell_word("docs/a b.md"), "'docs/a b.md'");
		assert_eq!(shell_word("/db/*.sql"), "'/db/*.sql'");
		assert_eq!(shell_word("src/[id]/página.ts"), "'src/[id]/página.ts'");
		// A quotation mark that would end a single-quoted word, in either
		// shell, takes double quotes.
		assert_eq!(shell_word("it's"), "\"it's\"");
		assert_eq!(shell_word("guide’s notes.md"), "\"guide’s notes.md\"");
		// Only with something double quotes would expand as well, the POSIX
		// form remains.
		assert_eq!(shell_word("it's $5"), r"'it'\''s $5'");
	}

	#[test]
	fn plain_hunks_have_no_file_headers() {
		assert_eq!(
			plain_hunks("a\nb\n", "a\nc\n"),
			"@@ -1,2 +1,2 @@\n a\n-b\n+c\n"
		);
	}
}
