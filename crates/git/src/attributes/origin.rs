//! Finds the attribute line that set a path's `filter`, for the
//! unsupported-filter diagnostic:
//! `unsupported clean filter <name> (<attribute origin>)`.
//!
//! This follows git's `attr.c`: the attribute files form a stack, from
//! `$GIT_DIR/info/attributes` down through each `.gitattributes` from the
//! path's directory to the root, then the global and system files. Within a
//! file the last matching line wins, and macros expand only when set. As in
//! git, a symbolic link is followed for the info, global and system files,
//! and refused for a `.gitattributes` file in the working tree.

use std::{
	collections::{HashMap, HashSet},
	fs,
	path::{Path, PathBuf},
};

use source_sentry_core::ports::RepoPath;

use super::{
	State,
	wildmatch::{Flags, wildmatch},
};
use crate::presence::native;

/// Git ignores attribute lines this long or longer (`ATTR_MAX_LINE_LENGTH`).
const MAX_LINE_LENGTH: usize = 2048;
/// Bounds macro expansion, which git leaves to its "fill each attribute once"
/// rule; a cycle cannot recurse further than the number of macros anyway.
const MAX_MACRO_DEPTH: usize = 64;
/// The characters git treats as blanks between tokens.
const BLANKS: &[u8] = b" \t\r\n";

/// Where the attribute files are.
pub(crate) struct OriginContext<'a> {
	/// The working tree, which holds the `.gitattributes` files.
	pub work_dir: &'a Path,
	/// The common git directory, which holds `info/attributes`.
	pub common_dir: &'a Path,
	/// `core.ignorecase`, which makes patterns match case-insensitively.
	pub ignore_case: bool,
	/// The global attributes file (`core.attributesFile`), if any.
	pub global: Option<PathBuf>,
	/// The system attributes file, if any.
	pub system: Option<PathBuf>,
	/// Reads a `.gitattributes` file from the index, for a directory whose
	/// working-tree copy is missing, as in a sparse checkout.
	pub index_blob: &'a dyn Fn(&RepoPath) -> Option<Vec<u8>>,
}

/// Where the `filter` attribute of `path` was set to `driver`, written as
/// `<file>:<line>`, or `None` when no line can be identified with certainty.
pub(crate) fn locate_filter(
	context: &OriginContext<'_>,
	path: &RepoPath,
	driver: &str,
) -> Option<String> {
	match resolve_filter(context, path) {
		// Git and this reading disagree if the line sets anything else;
		// naming it would then mislead.
		FilterLine::Line { origin, state } if state.value() == Some(driver) => {
			Some(origin)
		}
		_ => None,
	}
}

/// What the attribute files say about a path's `filter` attribute.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum FilterLine {
	/// The line that decides it, written as `<file>:<line>`, and the state
	/// that line gives.
	Line {
		/// The line, as `<file>:<line>`.
		origin: String,
		/// The state it gives the attribute.
		state: State,
	},
	/// No line mentions `filter` for the path.
	NoLine,
	/// The reading cannot tell: macro expansion reached its depth limit
	/// before the deciding line was found, or an attribute file exists but
	/// cannot be read.
	Inconclusive,
}

/// Finds the line that decides `path`'s `filter` attribute.
pub(crate) fn resolve_filter(
	context: &OriginContext<'_>,
	path: &RepoPath,
) -> FilterLine {
	// A file that cannot be read may hold the deciding line, or a macro
	// that a line read elsewhere expands.
	let Ok(sources) = load_sources(context, path) else {
		return FilterLine::Inconclusive;
	};
	let macros = collect_macros(&sources);
	let mut filled = HashSet::new();
	let mut truncated = false;
	for source in &sources {
		for line in source.lines.iter().rev() {
			let Entry::Pattern { pattern, states } = &line.entry else {
				continue;
			};
			if !pattern_matches(pattern, &source.base, path, context.ignore_case) {
				continue;
			}
			let found = fill(states, &macros, &mut filled, 0, &mut truncated);
			// A macro left unexpanded may have set `filter` before anything
			// found after it.
			if truncated {
				return FilterLine::Inconclusive;
			}
			if let Some(state) = found {
				return FilterLine::Line {
					origin: format!("{}:{}", source.label, line.number),
					state,
				};
			}
		}
	}
	FilterLine::NoLine
}

/// One attribute file.
struct Source {
	/// How the diagnostic names the file.
	label: String,
	/// The directory the file's patterns are relative to.
	base: RepoPath,
	/// The meaningful lines.
	lines: Vec<Line>,
}

/// One meaningful line of an attribute file.
struct Line {
	/// The line number, counting from 1.
	number: usize,
	/// What the line says.
	entry: Entry,
}

/// What an attribute line says.
enum Entry {
	/// `[attr]<name> <states>` defines a macro.
	Macro {
		name: String,
		states: Vec<(String, State)>,
	},
	/// `<pattern> <states>` assigns attributes to matching paths.
	Pattern {
		pattern: Vec<u8>,
		states: Vec<(String, State)>,
	},
}

/// An attribute file that exists but cannot be read.
struct Unreadable;

/// Reads the attribute stack for `path`, highest precedence first.
fn load_sources(
	context: &OriginContext<'_>,
	path: &RepoPath,
) -> Result<Vec<Source>, Unreadable> {
	let mut sources = Vec::new();
	let info = context.common_dir.join("info").join("attributes");
	if let Some(bytes) = read_followed(&info)? {
		sources.push(Source::parse(
			display(context.work_dir, &info),
			RepoPath::ROOT,
			&bytes,
			true,
		));
	}
	let mut directory = path.parent();
	while let Some(current) = directory {
		if let Ok(file) = current.join(".gitattributes") {
			let bytes = read_in_tree(&native(context.work_dir, &file))?
				.or_else(|| (context.index_blob)(&file));
			if let Some(bytes) = bytes {
				let macros_allowed = current.is_root();
				sources.push(Source::parse(
					file.as_str().to_owned(),
					current.clone(),
					&bytes,
					macros_allowed,
				));
			}
		}
		directory = current.parent();
	}
	for file in [&context.global, &context.system].into_iter().flatten() {
		if let Some(bytes) = read_followed(file)? {
			sources.push(Source::parse(
				file.display().to_string(),
				RepoPath::ROOT,
				&bytes,
				true,
			));
		}
	}
	Ok(sources)
}

/// The contents of `$GIT_DIR/info/attributes`, the global or the system
/// attribute file, following a symbolic link as git does, or `None` when
/// the file is absent.
fn read_followed(path: &Path) -> Result<Option<Vec<u8>>, Unreadable> {
	match fs::read(path) {
		Ok(bytes) => Ok(Some(bytes)),
		Err(error) if is_absence(&error) => Ok(None),
		Err(_) => Err(Unreadable),
	}
}

/// The contents of a `.gitattributes` file in the working tree, or `None`
/// when the file is absent or is a symbolic link, which git refuses there
/// and replaces with the index's copy.
fn read_in_tree(path: &Path) -> Result<Option<Vec<u8>>, Unreadable> {
	match fs::symlink_metadata(path) {
		Ok(metadata) if metadata.is_symlink() => Ok(None),
		Ok(metadata) if metadata.is_file() => read_followed(path),
		Err(error) if is_absence(&error) => Ok(None),
		Ok(_) | Err(_) => Err(Unreadable),
	}
}

/// Whether `error` says that the file does not exist, which git reads as
/// an empty attribute file. After any other failure the contents are
/// unknown.
fn is_absence(error: &std::io::Error) -> bool {
	matches!(
		error.kind(),
		std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
	)
}

/// A path below the working tree, written relative to it with `/`; any other
/// path as it is.
fn display(work_dir: &Path, path: &Path) -> String {
	match path.strip_prefix(work_dir) {
		Ok(relative) => relative
			.components()
			.map(|component| component.as_os_str().to_string_lossy())
			.collect::<Vec<_>>()
			.join("/"),
		Err(_) => path.display().to_string(),
	}
}

impl Source {
	/// Parses an attribute file.
	fn parse(
		label: String,
		base: RepoPath,
		bytes: &[u8],
		macros_allowed: bool,
	) -> Self {
		let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
		let lines = bytes
			.split(|&byte| byte == b'\n')
			.enumerate()
			.filter_map(|(index, raw)| {
				parse_line(raw, macros_allowed).map(|entry| Line {
					number: index + 1,
					entry,
				})
			})
			.collect();
		Self { label, base, lines }
	}
}

/// `parse_attr_line`: a macro definition or a pattern line, or `None` for a
/// blank line, a comment, or a line git ignores with a warning.
fn parse_line(raw: &[u8], macros_allowed: bool) -> Option<Entry> {
	if raw.len() >= MAX_LINE_LENGTH {
		return None;
	}
	let line = skip_blanks(raw);
	if line.is_empty() || line[0] == b'#' {
		return None;
	}
	if let Some(rest) = line.strip_prefix(b"[attr]") {
		if !macros_allowed {
			return None;
		}
		let rest = skip_blanks(rest);
		let end = token_end(rest);
		let name = std::str::from_utf8(&rest[..end]).ok()?;
		if !is_valid_name(name) {
			return None;
		}
		return Some(Entry::Macro {
			name: name.to_owned(),
			states: parse_states(&rest[end..])?,
		});
	}
	let (pattern, rest) = match line.first() {
		Some(b'"') => unquote(line).unwrap_or_else(|| split_token(line)),
		_ => split_token(line),
	};
	let states = parse_states(rest)?;
	if pattern.first() == Some(&b'!') {
		// Git rejects negative patterns in attribute files.
		return None;
	}
	Some(Entry::Pattern { pattern, states })
}

/// `line` without leading blanks.
fn skip_blanks(line: &[u8]) -> &[u8] {
	let start = line
		.iter()
		.position(|byte| !BLANKS.contains(byte))
		.unwrap_or(line.len());
	&line[start..]
}

/// The length of the token at the start of `line`.
fn token_end(line: &[u8]) -> usize {
	line
		.iter()
		.position(|byte| BLANKS.contains(byte))
		.unwrap_or(line.len())
}

/// Splits an unquoted pattern from the states after it.
fn split_token(line: &[u8]) -> (Vec<u8>, &[u8]) {
	let end = token_end(line);
	(line[..end].to_vec(), &line[end..])
}

/// `unquote_c_style`: decodes a quoted pattern and returns it with the text
/// after the closing quote, or `None` when the quoting is malformed.
fn unquote(line: &[u8]) -> Option<(Vec<u8>, &[u8])> {
	let mut pattern = Vec::new();
	let mut index = 1;
	loop {
		let byte = *line.get(index)?;
		index += 1;
		match byte {
			b'"' => return Some((pattern, &line[index..])),
			b'\\' => {
				let escaped = *line.get(index)?;
				index += 1;
				let decoded = match escaped {
					b'a' => 0x07,
					b'b' => 0x08,
					b'f' => 0x0c,
					b'n' => b'\n',
					b'r' => b'\r',
					b't' => b'\t',
					b'v' => 0x0b,
					b'\\' | b'"' => escaped,
					b'0'..=b'3' => {
						let second = *line.get(index)?;
						let third = *line.get(index + 1)?;
						if !(b'0'..=b'7').contains(&second)
							|| !(b'0'..=b'7').contains(&third)
						{
							return None;
						}
						index += 2;
						((escaped - b'0') << 6) | ((second - b'0') << 3) | (third - b'0')
					}
					_ => return None,
				};
				pattern.push(decoded);
			}
			_ => pattern.push(byte),
		}
	}
}

/// `parse_attr`, for every token: `name`, `-name`, `!name` or `name=value`.
/// Returns `None` when any name is invalid, because git then ignores the
/// whole line.
fn parse_states(mut rest: &[u8]) -> Option<Vec<(String, State)>> {
	let mut states = Vec::new();
	loop {
		rest = skip_blanks(rest);
		if rest.is_empty() {
			return Some(states);
		}
		let end = token_end(rest);
		let token = std::str::from_utf8(&rest[..end]).ok()?;
		rest = &rest[end..];
		let (name, state) = if let Some(name) = token.strip_prefix('-') {
			(name.split('=').next().unwrap_or(name), State::Unset)
		} else if let Some(name) = token.strip_prefix('!') {
			(name.split('=').next().unwrap_or(name), State::Unspecified)
		} else if let Some((name, value)) = token.split_once('=') {
			(name, State::Value(value.to_owned()))
		} else {
			(token, State::Set)
		};
		if !is_valid_name(name) {
			return None;
		}
		states.push((name.to_owned(), state));
	}
}

/// `attr_name_valid` without reserved names: letters, digits, `-`, `.` and
/// `_`, not starting with `-`, and not in the `builtin_` namespace.
fn is_valid_name(name: &str) -> bool {
	!name.is_empty()
		&& !name.starts_with('-')
		&& !name.starts_with("builtin_")
		&& name.bytes().all(|byte| {
			byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_')
		})
}

/// The macro definitions in effect: for each name, the highest-precedence
/// definition, then git's built-in `binary`.
fn collect_macros(sources: &[Source]) -> HashMap<String, Vec<(String, State)>> {
	let mut macros = HashMap::new();
	for source in sources {
		for line in source.lines.iter().rev() {
			if let Entry::Macro { name, states } = &line.entry {
				macros.entry(name.clone()).or_insert_with(|| states.clone());
			}
		}
	}
	macros.entry("binary".to_owned()).or_insert_with(|| {
		["diff", "merge", "text"]
			.into_iter()
			.map(|name| (name.to_owned(), State::Unset))
			.collect()
	});
	macros
}

/// `fill_one`: assigns a line's states, last first, to attributes that no
/// higher-precedence line has assigned, expanding macros that are set.
/// Returns the state the line gives `filter`, if it is the line that decides
/// it. Sets `truncated`, and stops, at a macro beyond [`MAX_MACRO_DEPTH`].
fn fill(
	states: &[(String, State)],
	macros: &HashMap<String, Vec<(String, State)>>,
	filled: &mut HashSet<String>,
	depth: usize,
	truncated: &mut bool,
) -> Option<State> {
	for (name, state) in states.iter().rev() {
		if !filled.insert(name.clone()) {
			continue;
		}
		if name == "filter" {
			return Some(state.clone());
		}
		if *state == State::Set
			&& let Some(definition) = macros.get(name)
		{
			if depth >= MAX_MACRO_DEPTH {
				*truncated = true;
				return None;
			}
			let found = fill(definition, macros, filled, depth + 1, truncated);
			if found.is_some() || *truncated {
				return found;
			}
		}
	}
	None
}

/// `path_matches` for a file: a pattern without a slash matches the base
/// name; any other pattern matches the path relative to the file's
/// directory. A pattern with a trailing slash matches only directories.
fn pattern_matches(
	pattern: &[u8],
	base: &RepoPath,
	path: &RepoPath,
	ignore_case: bool,
) -> bool {
	if pattern.last() == Some(&b'/') {
		return false;
	}
	if !pattern.contains(&b'/') {
		let name = path.file_name().unwrap_or_default();
		return wildmatch(
			pattern,
			name.as_bytes(),
			Flags {
				pathname: false,
				casefold: ignore_case,
			},
		);
	}
	let pattern = pattern.strip_prefix(b"/").unwrap_or(pattern);
	let Some(relative) = path.strip_prefix(base) else {
		return false;
	};
	wildmatch(
		pattern,
		relative.as_bytes(),
		Flags {
			pathname: true,
			casefold: ignore_case,
		},
	)
}

#[cfg(test)]
mod tests {
	use super::*;

	fn path(text: &str) -> RepoPath {
		RepoPath::new(text).unwrap()
	}

	fn locate(
		files: &[(&str, &str)],
		target: &str,
		driver: &str,
	) -> Option<String> {
		let work = tempfile::tempdir().unwrap();
		let common = work.path().join(".git");
		fs::create_dir_all(common.join("info")).unwrap();
		for (name, contents) in files {
			let full = work.path().join(name);
			fs::create_dir_all(full.parent().unwrap()).unwrap();
			fs::write(full, contents).unwrap();
		}
		let no_index = |_: &RepoPath| None;
		let context = OriginContext {
			work_dir: work.path(),
			common_dir: &common,
			ignore_case: false,
			global: None,
			system: None,
			index_blob: &no_index,
		};
		locate_filter(&context, &path(target), driver)
	}

	#[test]
	fn names_the_last_matching_line() {
		let files = [(
			".gitattributes",
			"# comment\n*.dat filter=one\n*.dat filter=other\n",
		)];
		assert_eq!(
			locate(&files, "x.dat", "other").as_deref(),
			Some(".gitattributes:3")
		);
	}

	#[test]
	fn prefers_deeper_files_and_info_attributes() {
		let files = [
			(".gitattributes", "*.dat filter=root\n"),
			("sub/.gitattributes", "\n*.dat filter=deep\n"),
		];
		assert_eq!(
			locate(&files, "sub/x.dat", "deep").as_deref(),
			Some("sub/.gitattributes:2")
		);
		let with_info = [
			(".gitattributes", "*.dat filter=root\n"),
			(".git/info/attributes", "*.dat filter=info\n"),
		];
		assert_eq!(
			locate(&with_info, "x.dat", "info").as_deref(),
			Some(".git/info/attributes:1")
		);
	}

	#[test]
	fn anchors_patterns_with_a_slash() {
		let files = [("sub/.gitattributes", "/x.dat filter=here\n")];
		assert_eq!(
			locate(&files, "sub/x.dat", "here").as_deref(),
			Some("sub/.gitattributes:1")
		);
		assert_eq!(locate(&files, "sub/deeper/x.dat", "here"), None);
	}

	#[test]
	fn expands_macros_that_are_set() {
		let files = [(
			".gitattributes",
			"[attr]custom filter=macro -text\n*.dat custom\n",
		)];
		assert_eq!(
			locate(&files, "x.dat", "macro").as_deref(),
			Some(".gitattributes:2")
		);
		let unset = [(
			".gitattributes",
			"[attr]custom filter=macro\n*.dat -custom\n",
		)];
		assert_eq!(locate(&unset, "x.dat", "macro"), None);
	}

	#[test]
	fn macros_nested_beyond_the_limit_are_inconclusive() {
		let chain = |levels: usize| {
			let mut lines: Vec<String> = (0..levels)
				.map(|level| format!("[attr]m{level} m{}", level + 1))
				.collect();
			lines.push(format!("[attr]m{levels} filter=deep"));
			lines.push("*.dat m0\n".to_owned());
			lines.join("\n")
		};
		let resolve = |text: &str| {
			let work = tempfile::tempdir().unwrap();
			let common = work.path().join(".git");
			fs::write(work.path().join(".gitattributes"), text).unwrap();
			let no_index = |_: &RepoPath| None;
			let context = OriginContext {
				work_dir: work.path(),
				common_dir: &common,
				ignore_case: false,
				global: None,
				system: None,
				index_blob: &no_index,
			};
			resolve_filter(&context, &path("x.dat"))
		};
		let within = chain(MAX_MACRO_DEPTH - 1);
		let lines = within.lines().count();
		assert_eq!(
			resolve(&within),
			FilterLine::Line {
				origin: format!(".gitattributes:{lines}"),
				state: State::Value("deep".to_owned()),
			}
		);
		assert_eq!(resolve(&chain(MAX_MACRO_DEPTH)), FilterLine::Inconclusive);
		assert_eq!(resolve("*.dat -text\n"), FilterLine::NoLine);
	}

	#[test]
	fn an_attribute_file_that_cannot_be_read_is_inconclusive() {
		let work = tempfile::tempdir().unwrap();
		let common = work.path().join(".git");
		fs::create_dir_all(work.path().join("sub/.gitattributes")).unwrap();
		let no_index = |_: &RepoPath| None;
		let context = OriginContext {
			work_dir: work.path(),
			common_dir: &common,
			ignore_case: false,
			global: Some(work.path().join("absent-global")),
			system: None,
			index_blob: &no_index,
		};
		assert_eq!(resolve_filter(&context, &path("x.dat")), FilterLine::NoLine);
		assert_eq!(
			resolve_filter(&context, &path("sub/x.dat")),
			FilterLine::Inconclusive
		);
	}

	#[test]
	fn reports_nothing_when_git_disagrees() {
		let files = [(".gitattributes", "*.dat filter=one\n")];
		assert_eq!(locate(&files, "x.dat", "two"), None);
		let reset = [(".gitattributes", "*.dat filter=one\n*.dat !filter\n")];
		assert_eq!(locate(&reset, "x.dat", "one"), None);
	}

	#[test]
	fn reads_quoted_patterns_and_skips_invalid_lines() {
		let files = [(
			".gitattributes",
			"\"x y.dat\" filter=quoted\n*.dat filter=bad -bad!name\n",
		)];
		assert_eq!(
			locate(&files, "x y.dat", "quoted").as_deref(),
			Some(".gitattributes:1")
		);
	}
}
