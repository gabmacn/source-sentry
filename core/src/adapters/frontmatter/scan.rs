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
		_ => key_of(content).map_or(Kind::Other, |key| Kind::Key(key == KEY)),
	}
}

/// The key of a line that starts a simple top-level mapping entry, such as
/// `title: x`, `'source-sentry':` or `"a b": c`, with single-quote escapes
/// resolved. `None` for any other line.
fn key_of(content: &[u8]) -> Option<Vec<u8>> {
	let (key, rest) = match content.first()? {
		b'\'' => single_quoted(&content[1..])?,
		b'"' => double_quoted(&content[1..])?,
		_ => return plain_key(content),
	};
	let rest = rest.trim_ascii_start();
	let after = rest.strip_prefix(b":")?;
	matches!(after.first(), None | Some(b' ' | b'\t')).then_some(key)
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

/// A double-quoted scalar's raw text and the bytes after its closing quote.
/// Escapes are kept as written: a key spelled with escapes is not recognized
/// as `source-sentry`, which no one writes.
fn double_quoted(content: &[u8]) -> Option<(Vec<u8>, &[u8])> {
	let mut index = 0;
	while index < content.len() {
		match content[index] {
			b'\\' => index += 2,
			b'"' => {
				return Some((content[..index].to_vec(), &content[index + 1..]));
			}
			_ => index += 1,
		}
	}
	None
}

/// The key of a line that starts with a plain scalar followed by `: `, or
/// `None` when the line cannot start one.
fn plain_key(content: &[u8]) -> Option<Vec<u8>> {
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
			return (!key.is_empty()).then(|| key.to_vec());
		}
	}
	None
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
pub(super) fn locate(bytes: &[u8]) -> Result<Located, FrontmatterError> {
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
	locate_block(bytes, &body, close.start, eol)
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
) -> Result<Located, FrontmatterError> {
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
		&& !matches!(body[first].1, Kind::Key(_))
	{
		let mention = body
			.iter()
			.position(|&(line, kind)| kind != Kind::Comment && mentions(line));
		if let Some(index) = mention {
			return Err(FrontmatterError::Unlocatable {
				line: line_number(index),
			});
		}
		return Ok(Located::NotAMapping {
			line: line_number(first),
		});
	}
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
			Kind::Other if mentions(line) => {
				return Err(FrontmatterError::Unlocatable {
					line: line_number(index),
				});
			}
			_ => {}
		}
	}
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
		assert_eq!(classify(b"? source-sentry"), Kind::Other);
		assert_eq!(classify(b"{source-sentry: {}}"), Kind::Other);
		assert_eq!(classify(b"&a source-sentry: {}"), Kind::Other);
		assert_eq!(classify(b"plain text"), Kind::Other);
		assert_eq!(classify(b"a #b: c"), Kind::Other);
		assert_eq!(classify(b"'unclosed: 1"), Kind::Other);
	}
}
