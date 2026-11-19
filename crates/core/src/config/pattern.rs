//! Path patterns, which are gitignore-style globs, and series patterns, which
//! are regular expressions.

use std::collections::BTreeSet;
use std::fmt;
use std::iter::Peekable;
use std::str::Chars;

use globset::{ErrorKind, GlobBuilder, GlobMatcher};
use regex::Regex;

use crate::model::RepoPath;

/// Why a pattern was rejected, and how to fix it when that is known.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PatternError {
	/// What is wrong with the pattern.
	pub(super) reason: String,
	/// A concrete correction.
	pub(super) correction: Option<String>,
}

impl PatternError {
	fn new(reason: impl Into<String>) -> Self {
		Self {
			reason: reason.into(),
			correction: None,
		}
	}

	fn correct(mut self, correction: impl Into<String>) -> Self {
		self.correction = Some(correction.into());
		self
	}
}

/// A gitignore-style glob, anchored at a directory.
///
/// The syntax is `.gitignore`'s: `*` and `?` never match `/`, `**` matches any
/// number of directories, a trailing `/` matches directories only, and a
/// pattern with a `/` at its start or in its middle is relative to the anchor,
/// while any other pattern matches at any depth below it. A leading `!` marks
/// an exception, which only ignore patterns may use, and `\!` or `\#` start a
/// pattern with a literal `!` or `#`. Braces are literal characters: `{a,b}`
/// is no alternation here, unlike in source globs. Matching is case-sensitive
/// on every platform, so that results never depend on the machine.
///
/// The anchor is the directory of the config that consumes the pattern: the
/// repository root for the repository config and the built-in defaults, the
/// config's directory for a directory config, and the importing config's
/// directory for a preset.
#[derive(Clone, Debug)]
pub struct PathPattern {
	source: String,
	anchor: RepoPath,
	negated: bool,
	directory_only: bool,
	matcher: GlobMatcher,
}

impl PathPattern {
	/// Compiles a pattern anchored at the repository root. Only ignore
	/// patterns pass `allow_negation`.
	pub(super) fn compile(
		source: &str,
		allow_negation: bool,
	) -> Result<Self, PatternError> {
		if source.trim().is_empty() {
			return Err(PatternError::new("the pattern is empty"));
		}
		let mut rest = source;
		let mut negated = false;
		if rest.starts_with("\\!") || rest.starts_with("\\#") {
			rest = &rest[1..];
		} else if let Some(exception) = rest.strip_prefix('!') {
			if !allow_negation {
				return Err(
					PatternError::new("only ignore patterns may start with `!`").correct(
						format!(
							"To exclude these paths, add {} to `documents.ignore`",
							quote(exception)
						),
					),
				);
			}
			negated = true;
			rest = exception;
		} else if rest.starts_with('#') {
			return Err(
				PatternError::new("a leading `#` starts a comment in .gitignore")
					.correct(format!(
						"Use {} to match a literal `#`",
						quote(&format!("\\{rest}"))
					)),
			);
		}
		rest = trim_trailing_spaces(rest);
		let anchored = rest.starts_with('/');
		if anchored {
			rest = &rest[1..];
		}
		let directory_only = rest.ends_with('/');
		if directory_only {
			rest = &rest[..rest.len() - 1];
			// An escaped trailing slash is still a slash (as in ripgrep #2236).
			rest = rest.strip_suffix('\\').unwrap_or(rest);
		}
		if rest.is_empty() {
			return Err(PatternError::new("the pattern matches no path"));
		}
		let mut glob = rest.to_owned();
		if !anchored && !rest.contains('/') && !rest.starts_with("**/") {
			glob = format!("**/{glob}");
		}
		// `dir/**` matches what is inside `dir`, but not `dir` itself.
		if glob.ends_with("/**") {
			glob.push_str("/*");
		}
		let glob = translate(&glob)?;
		let matcher = GlobBuilder::new(&glob)
			.literal_separator(true)
			.backslash_escape(true)
			.build()
			.map_err(|error| PatternError::new(error.kind().to_string()))?
			.compile_matcher();
		Ok(Self {
			source: source.to_owned(),
			anchor: RepoPath::ROOT,
			negated,
			directory_only,
			matcher,
		})
	}

	/// The same pattern, anchored at `anchor`.
	pub(super) fn anchored_at(&self, anchor: &RepoPath) -> Self {
		Self {
			anchor: anchor.clone(),
			..self.clone()
		}
	}

	/// The pattern as written, including any leading `!`.
	#[must_use]
	pub fn as_str(&self) -> &str {
		&self.source
	}

	/// The directory the pattern is relative to.
	#[must_use]
	pub fn anchor(&self) -> &RepoPath {
		&self.anchor
	}

	/// Whether the pattern is an exception, written with a leading `!`.
	#[must_use]
	pub fn is_negated(&self) -> bool {
		self.negated
	}

	/// Whether the pattern matches `path`, a file, or one of the directories
	/// that contain it below the anchor. A directory match covers everything
	/// inside the directory, as in `.gitignore`. Paths outside the anchor
	/// never match.
	///
	/// Negation does not change the answer: an exception "matches" the paths
	/// it brings back.
	#[must_use]
	pub fn matches(&self, path: &RepoPath) -> bool {
		let Some(relative) = path.strip_prefix(&self.anchor) else {
			return false;
		};
		if relative.is_empty() {
			return false;
		}
		if !self.directory_only && self.matcher.is_match(relative) {
			return true;
		}
		relative
			.match_indices('/')
			.any(|(end, _)| self.matcher.is_match(&relative[..end]))
	}
}

/// `pattern` without its trailing spaces, as in .gitignore: a space preceded
/// by a backslash is escaped and kept, with every space before it, and only
/// the unescaped spaces after the last escaped character are dropped. A
/// backslash escapes the character after it, so in `a\\ ` the backslashes
/// escape each other and the space is dropped.
fn trim_trailing_spaces(pattern: &str) -> &str {
	let mut end = 0;
	let mut characters = pattern.char_indices();
	while let Some((index, character)) = characters.next() {
		match character {
			' ' => {}
			'\\' => {
				end = characters
					.next()
					.map_or(pattern.len(), |(next, escaped)| next + escaped.len_utf8());
			}
			_ => end = index + character.len_utf8(),
		}
	}
	&pattern[..end]
}

/// The characters of a glob still to be read.
type Characters<'g> = Peekable<Chars<'g>>;

/// `glob`, read as `.gitignore` reads it, in the syntax of the glob library.
/// Every brace outside a character class is escaped, so that the matcher
/// reads braces, and the commas between them, as literal characters. Every
/// character class is rewritten so that the matcher reads the members that
/// `.gitignore` does. Escapes outside classes are copied as they are.
fn translate(glob: &str) -> Result<String, PatternError> {
	let mut translated = String::with_capacity(glob.len());
	let mut characters = glob.chars().peekable();
	while let Some(character) = characters.next() {
		match character {
			'\\' => {
				translated.push(character);
				translated.extend(characters.next());
			}
			'[' => read_class(&mut characters)?.write(&mut translated),
			'{' | '}' => {
				translated.push('\\');
				translated.push(character);
			}
			_ => translated.push(character),
		}
	}
	Ok(translated)
}

/// A character class: the ranges of characters it matches, or, when
/// negated, does not match.
struct Class {
	negated: bool,
	/// Inclusive ranges; a single character is a range from itself to itself.
	ranges: Vec<(char, char)>,
}

/// Reads the character class whose `[` was just read, as `.gitignore` reads
/// it. A leading `!` or `^` negates the class. A backslash makes the next
/// character a member, whatever it is. A `]` right after the opening, or
/// after the negation, is a member, and any other unescaped `]` closes the
/// class. Braces are members like any other character.
///
/// An unescaped `-` makes a range when a single member comes before it and a
/// character other than `]` after it; the end may be escaped. Any other `-`
/// is a member: one that starts or ends the class, and one right after a
/// range or a named class, so that `[a-c-e]` holds `a` to `c`, `-` and `e`.
/// A range whose end comes before its start is rejected, so that the mistake
/// is seen.
///
/// A `[:` starts a named class when the first `]` after it follows a `:`:
/// `[[:alpha:]]` is a class of every ASCII letter. Any other `[` is a member.
/// A name that git does not know is rejected, since git would match no path
/// with it.
fn read_class(characters: &mut Characters<'_>) -> Result<Class, PatternError> {
	let unclosed = || PatternError::new(ErrorKind::UnclosedClass.to_string());
	let negated = characters
		.next_if(|next| matches!(next, '!' | '^'))
		.is_some();
	let mut ranges: Vec<(char, char)> = Vec::new();
	let mut first = true;
	// The single member just read, which a `-` may start a range from.
	let mut previous: Option<char> = None;
	loop {
		let character = characters.next().ok_or_else(unclosed)?;
		let member = match character {
			']' if !first => break,
			'\\' => characters.next().ok_or_else(unclosed)?,
			'-' => match previous {
				Some(start) if characters.peek().is_some_and(|&next| next != ']') => {
					let mut end = characters.next().ok_or_else(unclosed)?;
					if end == '\\' {
						end = characters.next().ok_or_else(unclosed)?;
					}
					if end < start {
						return Err(PatternError::new(
							ErrorKind::InvalidRange(start, end).to_string(),
						));
					}
					if let Some(single) = ranges.last_mut() {
						single.1 = end;
					}
					previous = None;
					continue;
				}
				_ => '-',
			},
			'[' if characters.peek() == Some(&':') => {
				if let Some(named) = read_named_class(characters)? {
					ranges.extend_from_slice(named);
					previous = None;
					first = false;
					continue;
				}
				'['
			}
			member => member,
		};
		first = false;
		ranges.push((member, member));
		previous = Some(member);
	}
	Ok(Class { negated, ranges })
}

/// The ranges of the named class whose `[` was just read, with `:` next, and
/// moves past its closing `:]`. `None`, with nothing read, when the first `]`
/// does not follow a `:`, so that the `[` is an ordinary member.
///
/// The names and their members are those of git, which reads them in ASCII
/// only: `space`, for one, holds the tab, line feed, carriage return and
/// space, but neither the vertical tab nor the form feed.
fn read_named_class(
	characters: &mut Characters<'_>,
) -> Result<Option<&'static [(char, char)]>, PatternError> {
	let mut ahead = characters.clone();
	ahead.next();
	let mut text = String::new();
	loop {
		match ahead.next() {
			None => {
				return Err(PatternError::new(ErrorKind::UnclosedClass.to_string()));
			}
			Some(']') => break,
			Some(character) => text.push(character),
		}
	}
	let Some(name) = text.strip_suffix(':') else {
		return Ok(None);
	};
	let ranges: &'static [(char, char)] = match name {
		"alnum" => &[('0', '9'), ('A', 'Z'), ('a', 'z')],
		"alpha" => &[('A', 'Z'), ('a', 'z')],
		"blank" => &[('\t', '\t'), (' ', ' ')],
		"cntrl" => &[('\0', '\x1f'), ('\x7f', '\x7f')],
		"digit" => &[('0', '9')],
		"graph" => &[('!', '~')],
		"lower" => &[('a', 'z')],
		"print" => &[(' ', '~')],
		"punct" => &[('!', '/'), (':', '@'), ('[', '`'), ('{', '~')],
		"space" => &[('\t', '\n'), ('\r', '\r'), (' ', ' ')],
		"upper" => &[('A', 'Z')],
		"xdigit" => &[('0', '9'), ('A', 'F'), ('a', 'f')],
		_ => {
			return Err(
				PatternError::new(format!("`[:{name}:]` names no character class"))
					.correct(
						"Use one of `[:alnum:]`, `[:alpha:]`, `[:blank:]`, \
					 `[:cntrl:]`, `[:digit:]`, `[:graph:]`, `[:lower:]`, \
					 `[:print:]`, `[:punct:]`, `[:space:]`, `[:upper:]` or \
					 `[:xdigit:]`, or `\\[` for a literal `[`",
					),
			);
		}
	};
	*characters = ahead;
	Ok(Some(ranges))
}

impl Class {
	/// Writes the class in the glob library's syntax, which has no escapes
	/// inside a class: there, `]` is a member only first, `-` only first or
	/// last, and a leading `!` or `^` negates. So `]` and `-` are each taken
	/// out of any range that covers it, `]` is written first, `-` first
	/// without a `]` and last with one, and a class that is not negated never
	/// starts with `!` or `^`.
	fn write(&self, glob: &mut String) {
		/// The characters that only some positions of a class can hold, with
		/// the characters just before and after them.
		const PLACED: [(char, char, char); 2] = [(']', '\\', '^'), ('-', ',', '.')];
		let mut ranges = self.ranges.clone();
		let mut placed = [false; 2];
		for (index, &(character, before, after)) in PLACED.iter().enumerate() {
			let mut split = Vec::with_capacity(ranges.len());
			for (start, end) in ranges {
				if !(start..=end).contains(&character) {
					split.push((start, end));
					continue;
				}
				placed[index] = true;
				if start < character {
					split.push((start, before));
				}
				if character < end {
					split.push((after, end));
				}
			}
			ranges = split;
		}
		let [close, dash] = placed;
		if !self.negated && !close && !dash && !lead_with_a_plain_range(&mut ranges)
		{
			// Every member is `!` or `^`, and no class can say so.
			write_alternatives(&ranges, glob);
			return;
		}
		glob.push('[');
		if self.negated {
			glob.push('!');
		}
		if close {
			glob.push(']');
		} else if dash {
			glob.push('-');
		}
		for &(start, end) in &ranges {
			glob.push(start);
			if start < end {
				glob.push('-');
				glob.push(end);
			}
		}
		if close && dash {
			glob.push('-');
		}
		glob.push(']');
	}
}

/// Moves to the front of `ranges` one that starts with neither `!` nor `^`,
/// splitting off the first character of a range when that is needed. Returns
/// false when every range is a single `!` or `^`.
fn lead_with_a_plain_range(ranges: &mut Vec<(char, char)>) -> bool {
	let negates = |character: char| matches!(character, '!' | '^');
	if let Some(index) = ranges.iter().position(|&(start, _)| !negates(start)) {
		ranges.swap(0, index);
		return true;
	}
	let Some(index) = ranges.iter().position(|&(start, end)| start < end) else {
		return false;
	};
	let (start, end) = ranges[index];
	// The characters just after `!` and `^`.
	let next = if start == '!' { '"' } else { '_' };
	ranges[index] = (start, start);
	ranges.insert(0, (next, end));
	true
}

/// Writes `ranges`, each a single `!` or `^`, as escaped literals: one
/// alone, or the alternatives between both.
fn write_alternatives(ranges: &[(char, char)], glob: &mut String) {
	let members: BTreeSet<char> =
		ranges.iter().map(|&(start, _)| start).collect();
	if members.len() > 1 {
		glob.push('{');
	}
	for (index, member) in members.iter().enumerate() {
		if index > 0 {
			glob.push(',');
		}
		glob.push('\\');
		glob.push(*member);
	}
	if members.len() > 1 {
		glob.push('}');
	}
}

impl PartialEq for PathPattern {
	fn eq(&self, other: &Self) -> bool {
		self.source == other.source && self.anchor == other.anchor
	}
}

impl Eq for PathPattern {}

impl fmt::Display for PathPattern {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		formatter.write_str(&self.source)
	}
}

/// A series pattern: a regular expression with a capture group named `version`.
///
/// It is implicitly anchored: it must match a whole path relative to the
/// directory of the config that defines or imports the series.
#[derive(Clone, Debug)]
pub struct SeriesPattern {
	source: String,
	regex: Regex,
}

impl SeriesPattern {
	/// The name of the group that captures the version.
	pub const VERSION_GROUP: &str = "version";

	/// Compiles a pattern, anchored at both ends.
	pub(super) fn compile(source: &str) -> Result<Self, PatternError> {
		// The pattern on its own first, so that errors quote what was written.
		Regex::new(source).map_err(|error| PatternError::new(error.to_string()))?;
		let regex = Regex::new(&format!("^(?:{source})$"))
			.map_err(|error| PatternError::new(error.to_string()))?;
		let has_version = regex
			.capture_names()
			.flatten()
			.any(|name| name == Self::VERSION_GROUP);
		if !has_version {
			return Err(
				PatternError::new("it has no capture group named `version`").correct(
					"Capture the version with `(?P<version>...)`, as in \
					 'audit-(?P<version>\\d+)\\.md'",
				),
			);
		}
		Ok(Self {
			source: source.to_owned(),
			regex,
		})
	}

	/// The pattern as written.
	#[must_use]
	pub fn as_str(&self) -> &str {
		&self.source
	}

	/// The version that `relative` carries, when the whole of `relative`
	/// matches. `None` when it does not match, or when the `version` group
	/// did not take part in the match.
	#[must_use]
	pub fn version<'p>(&self, relative: &'p str) -> Option<&'p str> {
		self
			.regex
			.captures(relative)?
			.name(Self::VERSION_GROUP)
			.map(|version| version.as_str())
	}
}

impl PartialEq for SeriesPattern {
	fn eq(&self, other: &Self) -> bool {
		self.source == other.source
	}
}

impl Eq for SeriesPattern {}

impl fmt::Display for SeriesPattern {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		formatter.write_str(&self.source)
	}
}

/// Writes `value` as a single-quoted YAML scalar, the convention for globs and
/// regular expressions.
pub(crate) fn quote(value: &str) -> String {
	format!("'{}'", value.replace('\'', "''"))
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::testing::path;

	/// Whether `pattern`, as an ignore pattern, matches each of `names`.
	fn matches(pattern: &str, names: &[&str]) -> Vec<bool> {
		let pattern = PathPattern::compile(pattern, true)
			.unwrap_or_else(|error| panic!("`{pattern}` compiles: {error:?}"));
		names
			.iter()
			.map(|name| pattern.matches(&path(name)))
			.collect()
	}

	/// Whether the glob library, configured as for path patterns, compiles
	/// `glob` and matches each of `names`, or the error it gives.
	fn library(glob: &str, names: &[&str]) -> Result<Vec<bool>, String> {
		let matcher = GlobBuilder::new(glob)
			.literal_separator(true)
			.backslash_escape(true)
			.build()
			.map_err(|error| error.kind().to_string())?
			.compile_matcher();
		Ok(names.iter().map(|name| matcher.is_match(name)).collect())
	}

	/// The glob library's own reading of a class, which the translation
	/// relies on: no escapes inside a class, `]` a member only first, `-` a
	/// member only first or last, a leading `!` or `^` negating, braces and
	/// commas plain members.
	#[test]
	fn the_glob_library_reads_classes_as_the_translation_expects() {
		for (glob, names, expected) in [
			(r"[\x]", &["x", "\\x"][..], &[true, false][..]),
			(r"[\-a]", &["-", "]", "a", "b"], &[false, true, true, false]),
			("[]]", &["]"], &[true]),
			("[]a]", &["]", "a"], &[true, true]),
			("[a]]", &["a", "a]", "]"], &[false, true, false]),
			("[-a]", &["-", "a"], &[true, true]),
			("[a-]", &["-", "a"], &[true, true]),
			("[]-a]", &["]", "^", "a", "-"], &[true, true, true, false]),
			("[]-]", &["]", "-", "^"], &[true, true, false]),
			("[!a]", &["!", "a", "b"], &[true, false, true]),
			("[^a]", &["^", "a", "b"], &[true, false, true]),
			("[!!]", &["!", "a"], &[false, true]),
			("[!]a]", &["]", "a", "b"], &[false, false, true]),
			("[!-a]", &["-", "a", "b"], &[false, false, true]),
			("[{},]", &["{", "}", ","], &[true, true, true]),
			("[a-c-e]", &["a", "d", "e", "-"], &[true, true, true, false]),
			("[a-c-]", &["a", "-", "d"], &[true, true, false]),
			("[[a]", &["[", "a", ":"], &[true, true, false]),
			("[[:a]", &["[", ":", "a", "]"], &[true, true, true, false]),
			(r"\!", &["!"], &[true]),
			(r"{\!,\^}", &["!", "^", "a"], &[true, true, false]),
		] {
			assert_eq!(library(glob, names), Ok(expected.to_vec()), "{glob}");
		}
		assert_eq!(
			library("[ab", &[]),
			Err("unclosed character class; missing ']'".to_owned())
		);
		assert_eq!(
			library("[c-a]", &[]),
			Err("invalid range; 'c' > 'a'".to_owned())
		);
	}

	/// Each pattern's answers are what `git check-ignore --no-index` gives
	/// for the same line in a `.gitignore`.
	#[test]
	#[expect(
		clippy::too_many_lines,
		reason = "one table of patterns and git's answers reads best whole"
	)]
	fn character_classes_match_as_in_gitignore() {
		for (pattern, names, expected) in [
			(
				r"[\]{]x.md",
				&["]x.md", "{x.md", "x.md", "[x.md", "bx.md"][..],
				&[true, true, false, false, false][..],
			),
			(
				"[]]",
				&["]", "a", "[]]", "]]"],
				&[true, false, false, false],
			),
			("[!]]", &["]", "a", "!", "^"], &[false, true, true, true]),
			(
				r"[\\]",
				&["a", "]", "[", "b"],
				&[false, false, false, false],
			),
			(
				r"[a\-z]",
				&["a", "-", "z", "b", "]", "^", "_"],
				&[true, true, true, false, false, false, false],
			),
			("[{}]", &["{", "}", "{}", ","], &[true, true, false, false]),
			(
				"{a,b}",
				&["{a,b}", "a", "b", "{a", "a,b"],
				&[true, false, false, false, false],
			),
			("a{b", &["a{b"], &[true]),
			(r"[\!a]", &["!", "a", "b"], &[true, true, false]),
			(r"[\^]", &["^", "a"], &[true, false]),
			(r"[\!\^]", &["!", "^", "a"], &[true, true, false]),
			(
				"[]-a]",
				&["]", "^", "a", "b", "-"],
				&[true, true, true, false, false],
			),
			(
				r"[\]-a]",
				&["]", "^", "a", "b", "-"],
				&[true, true, true, false, false],
			),
			("[x-]", &["x", "-", "y"], &[true, true, false]),
			(r"[a-\z]", &["a", "m", "z", "-"], &[true, true, true, false]),
			(r"[!\-]", &["-", "a"], &[false, true]),
			("[!-a]", &["-", "a", "b", "!"], &[false, false, true, true]),
			(
				"[a-c]",
				&["a", "b", "c", "d", "-"],
				&[true, true, true, false, false],
			),
			(
				"[[:alnum:]]",
				&["a", "0", "Z", "_", "-"],
				&[true, true, true, false, false],
			),
			(
				"[[:alpha:]]",
				&["a", "z", "A", "Z", "0", "_", "a]", "é"],
				&[true, true, true, true, false, false, false, false],
			),
			("[[:blank:]]", &[" ", "a", "_"], &[true, false, false]),
			("[[:cntrl:]]", &["a", " ", "~"], &[false, false, false]),
			("[[:digit:]]", &["0", "9", "a"], &[true, true, false]),
			(
				"[[:graph:]]",
				&["a", "!", "~", "-", "]", " "],
				&[true, true, true, true, true, false],
			),
			("[[:lower:]]", &["a", "z", "A"], &[true, true, false]),
			("[[:print:]]", &[" ", "a", "~"], &[true, true, true]),
			(
				"[[:punct:]]",
				&["!", "-", "[", "]", "^", "_", "`", "{", "~", "a", "0"],
				&[
					true, true, true, true, true, true, true, true, true, false, false,
				],
			),
			(
				"ab[[:punct:]]",
				&["ab.", "ab:", "abc"],
				&[true, true, false],
			),
			("[[:space:]]", &[" ", "a"], &[true, false]),
			("[[:upper:]]", &["A", "Z", "a"], &[true, true, false]),
			(
				"[[:xdigit:]]",
				&["0", "9", "a", "f", "A", "F", "g", "G"],
				&[true, true, true, true, true, true, false, false],
			),
			(
				"[![:alpha:]]",
				&["0", "-", "]", "!", "a", "Z"],
				&[true, true, true, true, false, false],
			),
			(
				"ab[![:alpha:]]",
				&["ab/c", "ab.", "abc"],
				&[false, true, false],
			),
			("[^[:digit:]]", &["0", "a", "^"], &[false, true, true]),
			(
				"[a[:digit:]-]",
				&["a", "5", "-", "b"],
				&[true, true, true, false],
			),
			(
				"[[:digit:]-z]",
				&["5", "-", "z", "y"],
				&[true, true, true, false],
			),
			(
				"[[:upper:][:digit:]]",
				&["A", "5", "a"],
				&[true, true, false],
			),
			(
				"[A-[:alpha:]]",
				&["A]", "Z]", "[]", ":]", "l]", "b]", "a"],
				&[true, true, true, true, true, false, false],
			),
			(
				"ab[[:]]",
				&["ab[]", "ab:]", "ab]", "ab["],
				&[true, true, false, false],
			),
			(
				"ab[[:alpha]]",
				&["aba]", "ab[]", "ab:]", "aba", "abb]"],
				&[true, true, true, false, false],
			),
			(
				r"[\[:digit:]]",
				&["[]", "d]", "1", "]"],
				&[true, true, false, false],
			),
			(
				"[a-c-e]",
				&["a", "b", "c", "d", "e", "-"],
				&[true, true, true, false, true, true],
			),
			(
				"[!a-c-e]",
				&["a", "b", "c", "d", "e", "f", "-"],
				&[false, false, false, true, false, true, false],
			),
			("[a-]", &["a", "-", "b"], &[true, true, false]),
			("[-a]", &["a", "-", "b"], &[true, true, false]),
			("[--0]", &["-", "0", "1", ","], &[true, true, false, false]),
			(
				"ab[--0]",
				&["ab.", "ab0", "ab-", "ab1"],
				&[true, true, true, false],
			),
			(
				"[a-c--0]",
				&["a", "b", "-", "0", "d"],
				&[true, true, true, true, false],
			),
		] {
			assert_eq!(matches(pattern, names), expected, "{pattern}");
		}
	}

	#[test]
	fn classes_are_translated_into_the_glob_library_s_spelling() {
		for (pattern, glob) in [
			(r"[\]{]x.md", "[]{]x.md"),
			(r"[\\]", r"[\]"),
			(r"[a\-z]", "[-az]"),
			(r"[\!a]", "[a!]"),
			(r"[\!-#]", "[\"-#!]"),
			(r"[\^]", r"\^"),
			(r"[\!\^]", r"{\!,\^}"),
			(r"[\]-a]", "[]^-a]"),
			(r"[]\-]", "[]-]"),
			(r"[!\-]", "[!-]"),
			("[{}]{a,b}", r"[{}]\{a,b\}"),
			("[a-c]", "[a-c]"),
			("[[:alpha:]]", "[A-Za-z]"),
			("[![:digit:]]", "[!0-9]"),
			("[[:punct:]]", "[]!-,.-/:-@[-\\^-`{-~-]"),
			("[a-c-e]", "[-a-ce]"),
			("[--0]", "[-.-0]"),
			("[[:alpha]]", "[[:alpha]]"),
		] {
			assert_eq!(translate(pattern), Ok(glob.to_owned()), "{pattern}");
		}
	}

	/// Git matches no path with an unclosed class; the pattern is rejected
	/// instead, so that the mistake is seen. A `\]` does not close a class.
	#[test]
	fn an_unclosed_class_is_rejected() {
		for pattern in ["[ab", r"[a\]", r"[a\"] {
			let error = PathPattern::compile(pattern, true)
				.expect_err("an unclosed class is rejected");
			assert_eq!(
				error.reason, "unclosed character class; missing ']'",
				"{pattern}"
			);
		}
	}

	/// Git matches no path with a class name it does not know, and names are
	/// case-sensitive; the pattern is rejected instead, so that the mistake
	/// is seen.
	#[test]
	fn an_unknown_class_name_is_rejected() {
		for (pattern, name) in [
			("[[:word:]]", "word"),
			("[[:ALPHA:]]", "ALPHA"),
			("[![:alpha:][:nope:]]", "nope"),
			("[[::]]", ""),
		] {
			let error = PathPattern::compile(pattern, true)
				.expect_err("an unknown class name is rejected");
			assert_eq!(
				error.reason,
				format!("`[:{name}:]` names no character class"),
				"{pattern}"
			);
			assert!(
				error
					.correction
					.is_some_and(|correction| correction.contains("`[:alpha:]`")),
				"{pattern}"
			);
		}
	}

	/// A named class must still be followed by the `]` that closes its
	/// class.
	#[test]
	fn a_named_class_left_open_is_rejected() {
		for pattern in ["[[:alpha:]", "[[:alpha:", "[a[:"] {
			let error = PathPattern::compile(pattern, true)
				.expect_err("an unclosed class is rejected");
			assert_eq!(
				error.reason, "unclosed character class; missing ']'",
				"{pattern}"
			);
		}
	}

	/// Each named class holds the ASCII characters that git gives it, with
	/// none beyond: these characters cannot be in a path, so the
	/// translation is checked instead of a match.
	#[test]
	fn named_classes_hold_git_s_control_characters() {
		for (pattern, glob) in [
			("[[:cntrl:]]", "[\0-\x1f\x7f]"),
			("[[:space:]]", "[\t-\n\r ]"),
			("[[:blank:]]", "[\t ]"),
		] {
			assert_eq!(translate(pattern), Ok(glob.to_owned()), "{pattern}");
		}
	}

	#[test]
	fn a_range_that_ends_before_it_starts_is_rejected() {
		let error = PathPattern::compile("[c-a]", true)
			.expect_err("a reversed range is rejected");
		assert_eq!(error.reason, "invalid range; 'c' > 'a'");
	}

	#[test]
	fn only_unescaped_trailing_spaces_are_dropped() {
		for (pattern, trimmed) in [
			(r"draft\  ", r"draft\ "),
			(r"a\ ", r"a\ "),
			(r"a\\ ", r"a\\"),
			("a  ", "a"),
			(r"a \  ", r"a \ "),
			(r"a\", r"a\"),
			("   ", ""),
		] {
			assert_eq!(trim_trailing_spaces(pattern), trimmed, "{pattern:?}");
		}
	}

	#[test]
	fn an_escaped_space_before_unescaped_ones_is_kept() {
		assert_eq!(
			matches(
				r"draft\  ",
				&["draft ", "draft", "draft  ", "draft /file.md"]
			),
			[true, false, false, true]
		);
		assert_eq!(matches(r"a\ ", &["a ", "a"]), [true, false]);
		assert_eq!(matches("a  ", &["a", "a ", "a  "]), [true, false, false]);
	}

	#[test]
	fn a_pattern_of_spaces_only_is_empty() {
		let error = PathPattern::compile("   ", true)
			.expect_err("a pattern of spaces is rejected");
		assert_eq!(error.reason, "the pattern is empty");
	}
}
