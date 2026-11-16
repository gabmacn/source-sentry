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
/// An unescaped `-` between two members makes a range, and a `-` that starts
/// or ends the class is a member. A `-` right after a range extends that
/// range, and a range whose end comes before its start is an error, as the
/// glob library reads them. A named class such as `[:alpha:]` is no
/// special spelling: `[[:alpha:]]` is a class of `[`, `:` and some letters,
/// followed by a literal `]`.
fn read_class(characters: &mut Characters<'_>) -> Result<Class, PatternError> {
	let unclosed = || PatternError::new(ErrorKind::UnclosedClass.to_string());
	let negated = characters
		.next_if(|next| matches!(next, '!' | '^'))
		.is_some();
	let mut ranges: Vec<(char, char)> = Vec::new();
	let mut first = true;
	let mut in_range = false;
	loop {
		let character = characters.next().ok_or_else(unclosed)?;
		let member = match character {
			'\\' => characters.next().ok_or_else(unclosed)?,
			']' if !first => break,
			'-' if !first && !in_range => {
				in_range = true;
				continue;
			}
			member => member,
		};
		first = false;
		match ranges.last_mut() {
			Some(range) if in_range => {
				range.1 = member;
				if range.1 < range.0 {
					return Err(PatternError::new(
						ErrorKind::InvalidRange(range.0, range.1).to_string(),
					));
				}
				in_range = false;
			}
			_ => ranges.push((member, member)),
		}
	}
	if in_range {
		ranges.push(('-', '-'));
	}
	Ok(Class { negated, ranges })
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
pub(super) fn quote(value: &str) -> String {
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

	/// A named class such as `[:alpha:]` is no special spelling, a `-` right
	/// after a range extends that range, and a range that ends before it
	/// starts is rejected.
	#[test]
	fn named_classes_and_ranges_keep_the_glob_library_s_reading() {
		assert_eq!(
			matches("[[:alpha:]]", &["a", "a]", ":]", "[]", "b]"]),
			[false, true, true, true, false]
		);
		assert_eq!(
			matches("[a-c-e]", &["a", "d", "e", "-"]),
			[true, true, true, false]
		);
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
