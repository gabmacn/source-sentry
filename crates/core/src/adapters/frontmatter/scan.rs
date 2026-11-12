//! Locates the frontmatter and the `source-sentry` block inside it, by line,
//! without parsing anything else.
//!
//! Frontmatter is a block delimited by `---` lines at the very start of the
//! file, after an optional UTF-8 byte order mark. It closes at the next `---`
//! or `...` line, which YAML itself reads as the end of a document. Both
//! delimiters may carry trailing spaces or tabs. Without a closing line there
//! is no frontmatter.
//!
//! The `source-sentry` block is the key's line plus every following line that
//! cannot start another top-level entry: blank lines, indented lines,
//! comments and `- ` sequence entries. Trailing blank lines and column-0
//! comments are left out, because they read as belonging to what follows.
//!
//! A line that continues a quoted scalar or a flow collection opened above
//! is part of that value, even at column 0, so it never starts an entry;
//! unless something opened is never closed, which is a mistake that the scan
//! then reads past as written. A double-quoted key is compared after its
//! escapes are resolved, as YAML reads it.
//!
//! The scan works on bytes, so other keys need not be valid UTF-8.

use std::ops::Range;

use super::FrontmatterError;

/// A UTF-8 byte order mark.
const BOM: &[u8] = b"\xEF\xBB\xBF";

/// The key this module looks for.
const KEY: &[u8] = b"source-sentry";

/// A line ending.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Eol {
	/// `\n`.
	Lf,
	/// `\r\n`.
	CrLf,
}

impl Eol {
	/// The line ending's bytes.
	pub(super) fn as_str(self) -> &'static str {
		match self {
			Self::Lf => "\n",
			Self::CrLf => "\r\n",
		}
	}
}

/// Where a document's declaration is, or would go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Located {
	/// The file has no frontmatter. A new one would start at `insert_at`,
	/// which is after any byte order mark.
	NoFrontmatter {
		/// Where new frontmatter would start.
		insert_at: usize,
		/// The file's first line ending, or LF when it has none.
		eol: Eol,
		/// Whether the file starts with a `+++` line, which marks TOML
		/// frontmatter that YAML frontmatter must not be put in front of.
		toml: bool,
	},
	/// The frontmatter is not a YAML block mapping at column 0, so it cannot
	/// take a key, and it does not mention `source-sentry`.
	NotAMapping {
		/// The first line with content.
		line: usize,
	},
	/// The frontmatter is a block mapping without the key. A new block would
	/// go at `insert_at`, the start of the closing delimiter.
	Absent {
		/// Where a new block would go.
		insert_at: usize,
		/// The opening delimiter's line ending.
		eol: Eol,
	},
	/// The key is present.
	Present {
		/// The block's bytes, from the start of the key's line to the end of
		/// its last line, including that line's ending.
		block: Range<usize>,
		/// The key's line.
		line: usize,
		/// The opening delimiter's line ending.
		eol: Eol,
	},
}

/// One line of the file.
#[derive(Clone, Copy, Debug)]
struct Line {
	/// The first byte.
	start: usize,
	/// The end of the content, before any `\r\n` or `\n`.
	content_end: usize,
	/// The end of the line, after its ending.
	end: usize,
}

impl Line {
	fn content(self, bytes: &[u8]) -> &[u8] {
		&bytes[self.start..self.content_end]
	}
}

/// The lines of `bytes`, starting at `start`.
fn lines(bytes: &[u8], start: usize) -> impl Iterator<Item = Line> + '_ {
	let mut next = start;
	std::iter::from_fn(move || {
		if next >= bytes.len() {
			return None;
		}
		let start = next;
		let end = bytes[start..]
			.iter()
			.position(|&byte| byte == b'\n')
			.map_or(bytes.len(), |offset| start + offset + 1);
		let mut content_end = end;
		if bytes[start..content_end].ends_with(b"\n") {
			content_end -= 1;
		}
		if bytes[start..content_end].ends_with(b"\r") {
			content_end -= 1;
		}
		next = end;
		Some(Line {
			start,
			content_end,
			end,
		})
	})
}

/// Whether `content` is `marker` followed only by spaces and tabs.
fn is_delimiter(content: &[u8], marker: &[u8]) -> bool {
	content
		.strip_prefix(marker)
		.is_some_and(|rest| rest.iter().all(|&byte| byte == b' ' || byte == b'\t'))
}

/// The line ending of `line`, or `None` when it is the last line and has
/// none.
fn eol_of(bytes: &[u8], line: Line) -> Option<Eol> {
	match &bytes[line.content_end..line.end] {
		b"\r\n" => Some(Eol::CrLf),
		b"\n" => Some(Eol::Lf),
		_ => None,
	}
}

/// What a line of the frontmatter can be, as far as top-level structure is
/// concerned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
	/// Empty, or only spaces and tabs.
	Blank,
	/// Starts with a space or tab: part of the entry above.
	Indented,
	/// A comment at column 0.
	Comment,
	/// A `- ` sequence entry at column 0, which continues the key above.
	Entry,
	/// A simple key at column 0. `true` when the key is `source-sentry`.
	Key(bool),
	/// A `? key` or `: value` line at column 0: part of an explicit entry of
	/// a block mapping, which is never the `source-sentry` block.
	Explicit,
	/// Anything else at column 0, such as `? key` or a flow collection.
	Other,
}

fn classify(content: &[u8]) -> Kind {
	match content {
		[] => Kind::Blank,
		[b' ' | b'\t', ..] => {
			if content.iter().all(|&byte| byte == b' ' || byte == b'\t') {
				Kind::Blank
			} else {
				Kind::Indented
			}
		}
		[b'#', ..] => Kind::Comment,
		[b'-'] | [b'-', b' ' | b'\t', ..] => Kind::Entry,
		[b'?' | b':'] | [b'?' | b':', b' ' | b'\t', ..] => Kind::Explicit,
		_ => key_of(content).map_or(Kind::Other, |(key, _)| Kind::Key(key == KEY)),
	}
}

/// The key of a line that starts a simple top-level mapping entry, such as
/// `title: x`, `'source-sentry':` or `"a b": c`, with quoting and escapes
/// resolved, and where its value starts. `None` for any other line.
fn key_of(content: &[u8]) -> Option<(Vec<u8>, usize)> {
	let (key, rest) = match content.first()? {
		b'\'' => single_quoted(&content[1..])?,
		b'"' => double_quoted(&content[1..])?,
		_ => return plain_key(content),
	};
	let rest = rest.trim_ascii_start();
	let after = rest.strip_prefix(b":")?;
	matches!(after.first(), None | Some(b' ' | b'\t'))
		.then(|| (key, content.len() - after.len()))
}

/// A single-quoted scalar's value and the bytes after its closing quote.
fn single_quoted(content: &[u8]) -> Option<(Vec<u8>, &[u8])> {
	let mut key = Vec::new();
	let mut index = 0;
	loop {
		let quote = index + content[index..].iter().position(|&b| b == b'\'')?;
		key.extend_from_slice(&content[index..quote]);
		if content.get(quote + 1) == Some(&b'\'') {
			key.push(b'\'');
			index = quote + 2;
		} else {
			return Some((key, &content[quote + 1..]));
		}
	}
}

/// A double-quoted scalar's value, with its escapes resolved, and the bytes
/// after its closing quote. A key spelled with escapes, such as
/// `"source-sentry"`, is therefore the key it decodes to, as YAML reads
/// it. `None` when the scalar does not close on the line, or an escape is
/// invalid.
fn double_quoted(content: &[u8]) -> Option<(Vec<u8>, &[u8])> {
	let mut value = Vec::new();
	let mut index = 0;
	while index < content.len() {
		match content[index] {
			b'\\' => {
				let (decoded, used) = escape(&content[index + 1..])?;
				let mut buffer = [0; 4];
				value.extend_from_slice(decoded.encode_utf8(&mut buffer).as_bytes());
				index += 1 + used;
			}
			b'"' => return Some((value, &content[index + 1..])),
			byte => {
				value.push(byte);
				index += 1;
			}
		}
	}
	None
}

/// The character a YAML double-quoted escape stands for, from the bytes after
/// its backslash, and how many of them it uses.
fn escape(after: &[u8]) -> Option<(char, usize)> {
	let hex = |digits: usize| {
		let text = std::str::from_utf8(after.get(1..=digits)?).ok()?;
		let code = u32::from_str_radix(text, 16).ok()?;
		Some((char::from_u32(code)?, 1 + digits))
	};
	let simple = |character: char| Some((character, 1));
	match after.first()? {
		b'x' => hex(2),
		b'u' => hex(4),
		b'U' => hex(8),
		b'0' => simple('\0'),
		b'a' => simple('\u{7}'),
		b'b' => simple('\u{8}'),
		b't' | b'\t' => simple('\t'),
		b'n' => simple('\n'),
		b'v' => simple('\u{b}'),
		b'f' => simple('\u{c}'),
		b'r' => simple('\r'),
		b'e' => simple('\u{1b}'),
		b' ' => simple(' '),
		b'"' => simple('"'),
		b'/' => simple('/'),
		b'\\' => simple('\\'),
		b'N' => simple('\u{85}'),
		b'_' => simple('\u{a0}'),
		b'L' => simple('\u{2028}'),
		b'P' => simple('\u{2029}'),
		_ => None,
	}
}

/// The key of a line that starts with a plain scalar followed by `: `, or
/// `None` when the line cannot start one.
fn plain_key(content: &[u8]) -> Option<(Vec<u8>, usize)> {
	let starts_plain = match content.first()? {
		b'[' | b']' | b'{' | b'}' | b',' | b'#' | b'&' | b'*' | b'!' | b'|'
		| b'>' | b'%' | b'@' | b'`' => false,
		b'?' | b':' | b'-' => !matches!(content.get(1), None | Some(b' ' | b'\t')),
		_ => true,
	};
	if !starts_plain {
		return None;
	}
	for (index, &byte) in content.iter().enumerate() {
		let after = content.get(index + 1);
		let before = index.checked_sub(1).map(|before| content[before]);
		if byte == b'#' && matches!(before, Some(b' ' | b'\t')) {
			return None;
		}
		if byte == b':' && matches!(after, None | Some(b' ' | b'\t')) {
			let key = content[..index].trim_ascii_end();
			return (!key.is_empty()).then(|| (key.to_vec(), index + 1));
		}
	}
	None
}

/// What a line leaves open: a quoted scalar or a flow collection that goes
/// on to the next line. YAML reads such a next line as part of the value,
/// even at column 0, so it can never start an entry.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Open {
	/// Nested `[` and `{` not yet closed.
	depth: usize,
	/// The quote of an unfinished quoted scalar, `"` or `'`.
	quote: Option<u8>,
}

impl Open {
	fn is_open(self) -> bool {
		self.depth > 0 || self.quote.is_some()
	}
}

/// What stays open after `text`. With `starts_value`, `text` is a value as
/// it follows `key:`, `- `, `? ` or `: `, which opens a quote or a flow
/// collection only with its first token, after any anchor or tag; otherwise
/// it continues what `open` holds.
fn advance(mut open: Open, text: &[u8], starts_value: bool) -> Open {
	let mut index = 0;
	if starts_value {
		loop {
			while text
				.get(index)
				.is_some_and(|byte| matches!(byte, b' ' | b'\t'))
			{
				index += 1;
			}
			if !matches!(text.get(index), Some(b'&' | b'!')) {
				break;
			}
			while text
				.get(index)
				.is_some_and(|byte| !matches!(byte, b' ' | b'\t'))
			{
				index += 1;
			}
		}
		match text.get(index) {
			Some(&quote @ (b'"' | b'\'')) => open.quote = Some(quote),
			Some(b'[' | b'{') => open.depth = 1,
			_ => return open,
		}
		index += 1;
	}
	// Whether a quote here would start a scalar, as after `[`, `,` or `: `.
	let mut token_start = true;
	while index < text.len() {
		let byte = text[index];
		match open.quote {
			Some(b'"') => match byte {
				b'\\' => index += 1,
				b'"' => {
					open.quote = None;
					token_start = false;
				}
				_ => {}
			},
			Some(_) => {
				if byte == b'\'' {
					if text.get(index + 1) == Some(&b'\'') {
						index += 1;
					} else {
						open.quote = None;
						token_start = false;
					}
				}
			}
			None if open.depth == 0 => break,
			None => match byte {
				b'"' | b'\'' if token_start => open.quote = Some(byte),
				b'[' | b'{' | b',' => {
					open.depth += usize::from(byte != b',');
					token_start = true;
				}
				b']' | b'}' => {
					open.depth -= 1;
					token_start = false;
				}
				b':' => token_start = true,
				b'#' if index == 0 || matches!(text[index - 1], b' ' | b'\t') => break,
				b' ' | b'\t' => {}
				_ => token_start = false,
			},
		}
		index += 1;
	}
	open
}

/// Marks as continuations the lines that continue a quoted scalar or a flow
/// collection left open by an entry above, so that none of them is taken
/// for a key.
///
/// Only when everything opened also closes before the frontmatter ends: an
/// unclosed quote or bracket is a mistake, after which the lines are read
/// as they were written rather than swallowed, so that a typo in another
/// key never hides the declaration.
fn mark_continuations(bytes: &[u8], body: &mut [(Line, Kind)]) {
	let mut open = Open::default();
	let mut continued = Vec::new();
	for (index, (line, kind)) in body.iter().enumerate() {
		let content = line.content(bytes);
		if open.is_open() {
			continued.push(index);
			open = advance(open, content, false);
		} else {
			let start = match kind {
				Kind::Key(_) => key_of(content).map(|(_, start)| start),
				Kind::Explicit | Kind::Entry => Some(1),
				_ => None,
			};
			if let Some(start) = start {
				open = advance(Open::default(), &content[start..], true);
			}
		}
	}
	if !open.is_open() {
		for index in continued {
			body[index].1 = Kind::Indented;
		}
	}
}

/// What a YAML parse of the whole frontmatter found, which the scan follows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Yaml {
	/// No parse: the scan decides alone, and refuses any line that names the
	/// key in a form it cannot locate.
	Unavailable,
	/// The frontmatter is a mapping without the key: lines that look like it
	/// belong to other values.
	NoKey,
	/// The mapping has the key on this file line, which must be a plain
	/// `source-sentry:` line for the scan to locate it.
	KeyAt(usize),
}

/// The 1-based line number of the frontmatter line at `index`, counting the
/// opening delimiter as line 1.
fn line_number(index: usize) -> usize {
	index + 2
}

/// Finds the frontmatter of `bytes` and the `source-sentry` block in it.
///
/// # Errors
///
/// Fails when the key appears twice, or when a line mentions
/// `source-sentry` in a form this scan cannot locate, such as a flow mapping
/// or an explicit `? source-sentry` key. Failing is safer than reading such
/// a document as undeclared.
///
/// With a [`Yaml`] parse, the scan takes the key's line from it: a line that
/// only looks like the key, inside a quoted or flow value, is ignored, and a
/// key the scan cannot locate is [`FrontmatterError::Ambiguous`].
pub(super) fn locate(
	bytes: &[u8],
	yaml: Yaml,
) -> Result<Located, FrontmatterError> {
	let start = if bytes.starts_with(BOM) { BOM.len() } else { 0 };
	let mut all = lines(bytes, start);
	let Some(open) = all.next() else {
		return Ok(no_frontmatter(bytes, start, None));
	};
	let Some(eol) =
		eol_of(bytes, open).filter(|_| is_delimiter(open.content(bytes), b"---"))
	else {
		return Ok(no_frontmatter(bytes, start, Some(open)));
	};
	let mut body = Vec::new();
	let mut close = None;
	for line in all {
		let content = line.content(bytes);
		if is_delimiter(content, b"---") || is_delimiter(content, b"...") {
			close = Some(line);
			break;
		}
		body.push((line, classify(content)));
	}
	let Some(close) = close else {
		return Ok(no_frontmatter(bytes, start, Some(open)));
	};
	mark_continuations(bytes, &mut body);
	locate_block(bytes, &body, close.start, eol, yaml)
}

/// The bytes between the frontmatter's delimiters, or `None` when `bytes` has
/// no frontmatter.
pub(super) fn frontmatter(bytes: &[u8]) -> Option<Range<usize>> {
	let start = if bytes.starts_with(BOM) { BOM.len() } else { 0 };
	let mut all = lines(bytes, start);
	let open = all.next()?;
	eol_of(bytes, open).filter(|_| is_delimiter(open.content(bytes), b"---"))?;
	all
		.find(|line| {
			let content = line.content(bytes);
			is_delimiter(content, b"---") || is_delimiter(content, b"...")
		})
		.map(|close| open.end..close.start)
}

/// The [`Located::NoFrontmatter`] value for a file whose first line, after
/// the byte order mark at `start`, is `first`.
fn no_frontmatter(bytes: &[u8], start: usize, first: Option<Line>) -> Located {
	let eol = lines(bytes, start)
		.find_map(|line| eol_of(bytes, line))
		.unwrap_or(Eol::Lf);
	Located::NoFrontmatter {
		insert_at: start,
		eol,
		toml: first.is_some_and(|line| is_delimiter(line.content(bytes), b"+++")),
	}
}

/// Finds the block among the classified frontmatter lines.
fn locate_block(
	bytes: &[u8],
	body: &[(Line, Kind)],
	close: usize,
	eol: Eol,
	yaml: Yaml,
) -> Result<Located, FrontmatterError> {
	let verified = yaml != Yaml::Unavailable;
	let mentions = |line: Line| {
		line
			.content(bytes)
			.windows(KEY.len())
			.any(|window| window == KEY)
	};
	let first = body
		.iter()
		.position(|(_, kind)| !matches!(kind, Kind::Blank | Kind::Comment));
	if let Some(first) = first
		&& !matches!(body[first].1, Kind::Key(_) | Kind::Explicit)
	{
		let mention = body
			.iter()
			.position(|&(line, kind)| kind != Kind::Comment && mentions(line))
			.filter(|_| !verified);
		if let Some(index) = mention {
			return Err(FrontmatterError::Unlocatable {
				line: line_number(index),
			});
		}
		if let Yaml::KeyAt(line) = yaml {
			return Err(FrontmatterError::Ambiguous { line: Some(line) });
		}
		return Ok(Located::NotAMapping {
			line: line_number(first),
		});
	}
	let found = match yaml {
		Yaml::Unavailable => scan_for_key(bytes, body)?,
		Yaml::NoKey => None,
		Yaml::KeyAt(line) => {
			let index = line.checked_sub(2).filter(|&index| {
				body
					.get(index)
					.is_some_and(|(_, kind)| *kind == Kind::Key(true))
			});
			if index.is_none() {
				return Err(FrontmatterError::Ambiguous { line: Some(line) });
			}
			index
		}
	};
	let Some(key) = found else {
		return Ok(Located::Absent {
			insert_at: close,
			eol,
		});
	};
	let mut end = key + 1;
	while body.get(end).is_some_and(|(_, kind)| {
		matches!(
			kind,
			Kind::Blank | Kind::Indented | Kind::Comment | Kind::Entry
		)
	}) {
		end += 1;
	}
	while end > key + 1 && matches!(body[end - 1].1, Kind::Blank | Kind::Comment)
	{
		end -= 1;
	}
	Ok(Located::Present {
		block: body[key].0.start..body[end - 1].0.end,
		line: line_number(key),
		eol,
	})
}

/// The index of the one `source-sentry:` line, without a YAML parse to
/// confirm it.
///
/// # Errors
///
/// Fails when the key appears twice, or when another line names it in a
/// form the scan cannot locate.
fn scan_for_key(
	bytes: &[u8],
	body: &[(Line, Kind)],
) -> Result<Option<usize>, FrontmatterError> {
	let mentions = |line: Line| {
		line
			.content(bytes)
			.windows(KEY.len())
			.any(|window| window == KEY)
	};
	let mut found: Option<usize> = None;
	for (index, &(line, kind)) in body.iter().enumerate() {
		match kind {
			Kind::Key(true) => {
				if let Some(first) = found {
					return Err(FrontmatterError::DuplicateKey {
						first: line_number(first),
						second: line_number(index),
					});
				}
				found = Some(index);
			}
			Kind::Other | Kind::Explicit if mentions(line) => {
				return Err(FrontmatterError::Unlocatable {
					line: line_number(index),
				});
			}
			_ => {}
		}
	}
	Ok(found)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn keys_are_recognized_in_every_simple_spelling() {
		assert_eq!(classify(b"source-sentry:"), Kind::Key(true));
		assert_eq!(classify(b"source-sentry: {}"), Kind::Key(true));
		assert_eq!(classify(b"source-sentry :"), Kind::Key(true));
		assert_eq!(classify(b"'source-sentry':"), Kind::Key(true));
		assert_eq!(classify(b"\"source-sentry\": # c"), Kind::Key(true));
		assert_eq!(classify(b"source-sentry-x: 1"), Kind::Key(false));
		assert_eq!(classify(b"'it''s': 1"), Kind::Key(false));
		assert_eq!(classify(b"title: a: b"), Kind::Key(false));
		assert_eq!(classify(b"url: http://x"), Kind::Key(false));
	}

	#[test]
	fn other_lines_are_classified_by_their_first_byte() {
		assert_eq!(classify(b""), Kind::Blank);
		assert_eq!(classify(b" \t"), Kind::Blank);
		assert_eq!(classify(b"  a: 1"), Kind::Indented);
		assert_eq!(classify(b"# source-sentry:"), Kind::Comment);
		assert_eq!(classify(b"- a"), Kind::Entry);
		assert_eq!(classify(b"-"), Kind::Entry);
		assert_eq!(classify(b"? source-sentry"), Kind::Explicit);
		assert_eq!(classify(b": source-sentry"), Kind::Explicit);
		assert_eq!(classify(b"?"), Kind::Explicit);
		assert_eq!(classify(b"?x: 1"), Kind::Key(false));
		assert_eq!(classify(b"{source-sentry: {}}"), Kind::Other);
		assert_eq!(classify(b"&a source-sentry: {}"), Kind::Other);
		assert_eq!(classify(b"plain text"), Kind::Other);
		assert_eq!(classify(b"a #b: c"), Kind::Other);
		assert_eq!(classify(b"'unclosed: 1"), Kind::Other);
	}
}
