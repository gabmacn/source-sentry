//! A port of git's `wildmatch.c`, which matches gitattributes patterns.
//!
//! It works on bytes, as git does. Out-of-range reads yield `0`, standing in
//! for C's terminating NUL, so the control flow mirrors the original closely
//! enough to compare line by line.

/// The result of one matching attempt, as in `wildmatch.c`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
	Match,
	NoMatch,
	AbortAll,
	AbortToStarStar,
}

/// Matching options.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Flags {
	/// `WM_PATHNAME`: wildcards do not match `/`, and `**` spans directories.
	pub pathname: bool,
	/// `WM_CASEFOLD`: ASCII letters match case-insensitively.
	pub casefold: bool,
}

/// Whether `text` matches the wildcard `pattern`.
pub(crate) fn wildmatch(pattern: &[u8], text: &[u8], flags: Flags) -> bool {
	matches(pattern, 0, text, 0, flags) == Outcome::Match
}

/// The byte at `index`, or `0` past the end.
fn at(bytes: &[u8], index: usize) -> u8 {
	bytes.get(index).copied().unwrap_or(0)
}

/// Whether a `/` occurs in `text` from `start`, stopping at a NUL.
fn slash_from(text: &[u8], start: usize) -> Option<usize> {
	text
		.get(start..)?
		.iter()
		.take_while(|&&byte| byte != 0)
		.position(|&byte| byte == b'/')
		.map(|offset| start + offset)
}

/// Lowercases an ASCII letter when folding case.
fn fold(byte: u8, flags: Flags) -> u8 {
	if flags.casefold {
		byte.to_ascii_lowercase()
	} else {
		byte
	}
}

/// `is_glob_special`: the characters `*?[\`.
fn is_glob_special(byte: u8) -> bool {
	matches!(byte, b'*' | b'?' | b'[' | b'\\')
}

/// `dowild`: matches `pattern[p..]` against `text[t..]`.
#[expect(
	clippy::too_many_lines,
	reason = "a faithful port of one C function reads best as one function"
)]
fn matches(
	pattern: &[u8],
	mut p: usize,
	text: &[u8],
	mut t: usize,
	flags: Flags,
) -> Outcome {
	while at(pattern, p) != 0 {
		let mut p_ch = at(pattern, p);
		let mut t_ch = at(text, t);
		if t_ch == 0 && p_ch != b'*' {
			return Outcome::AbortAll;
		}
		t_ch = fold(t_ch, flags);
		p_ch = fold(p_ch, flags);
		match p_ch {
			b'?' => {
				if flags.pathname && t_ch == b'/' {
					return Outcome::NoMatch;
				}
			}
			b'*' => {
				let match_slash;
				p += 1;
				if at(pattern, p) == b'*' {
					let before = p.checked_sub(2);
					p += 1;
					while at(pattern, p) == b'*' {
						p += 1;
					}
					let at_start = before.is_none_or(|index| at(pattern, index) == b'/');
					let next = at(pattern, p);
					if at_start
						&& (next == 0
							|| next == b'/'
							|| (next == b'\\' && at(pattern, p + 1) == b'/'))
					{
						if next == b'/'
							&& matches(pattern, p + 1, text, t, flags) == Outcome::Match
						{
							return Outcome::Match;
						}
						match_slash = true;
					} else {
						match_slash = false;
					}
				} else {
					match_slash = !flags.pathname;
				}
				if at(pattern, p) == 0 {
					// A trailing `**` matches everything; a trailing `*` only
					// when no slash follows.
					if !match_slash && slash_from(text, t).is_some() {
						return Outcome::NoMatch;
					}
					return Outcome::Match;
				} else if !match_slash && at(pattern, p) == b'/' {
					// One `*` followed by a slash matches the next directory.
					let Some(slash) = slash_from(text, t) else {
						return Outcome::NoMatch;
					};
					t = slash;
					// The slash is consumed by the loop's increment below.
					p += 1;
					t += 1;
					continue;
				}
				loop {
					if t_ch == 0 {
						break;
					}
					let next = at(pattern, p);
					if !is_glob_special(next) {
						let p_ch = fold(next, flags);
						loop {
							t_ch = at(text, t);
							if t_ch == 0 || (!match_slash && t_ch == b'/') {
								break;
							}
							t_ch = fold(t_ch, flags);
							if t_ch == p_ch {
								break;
							}
							t += 1;
						}
						if t_ch != p_ch {
							return Outcome::NoMatch;
						}
					}
					let matched = matches(pattern, p, text, t, flags);
					if matched != Outcome::NoMatch {
						if !match_slash || matched != Outcome::AbortToStarStar {
							return matched;
						}
					} else if !match_slash && t_ch == b'/' {
						return Outcome::AbortToStarStar;
					}
					t += 1;
					t_ch = at(text, t);
				}
				return Outcome::AbortAll;
			}
			b'[' => {
				p += 1;
				p_ch = at(pattern, p);
				if p_ch == b'^' {
					p_ch = b'!';
				}
				let negated = p_ch == b'!';
				if negated {
					p += 1;
					p_ch = at(pattern, p);
				}
				let mut prev_ch = 0;
				let mut matched = false;
				loop {
					if p_ch == 0 {
						return Outcome::AbortAll;
					}
					if p_ch == b'\\' {
						p += 1;
						p_ch = at(pattern, p);
						if p_ch == 0 {
							return Outcome::AbortAll;
						}
						if t_ch == p_ch {
							matched = true;
						}
					} else if p_ch == b'-'
						&& prev_ch != 0
						&& at(pattern, p + 1) != 0
						&& at(pattern, p + 1) != b']'
					{
						p += 1;
						p_ch = at(pattern, p);
						if p_ch == b'\\' {
							p += 1;
							p_ch = at(pattern, p);
							if p_ch == 0 {
								return Outcome::AbortAll;
							}
						}
						if t_ch <= p_ch && t_ch >= prev_ch {
							matched = true;
						} else if flags.casefold && t_ch.is_ascii_lowercase() {
							let upper = t_ch.to_ascii_uppercase();
							if upper <= p_ch && upper >= prev_ch {
								matched = true;
							}
						}
						p_ch = 0;
					} else if p_ch == b'[' && at(pattern, p + 1) == b':' {
						p += 2;
						let start = p;
						while at(pattern, p) != 0 && at(pattern, p) != b']' {
							p += 1;
						}
						if at(pattern, p) == 0 {
							return Outcome::AbortAll;
						}
						if p == start || at(pattern, p - 1) != b':' {
							// No closing `:]`: treat the `[` as an ordinary
							// member of the set.
							p = start - 2;
							p_ch = b'[';
							if t_ch == p_ch {
								matched = true;
							}
						} else {
							match class_matches(&pattern[start..p - 1], t_ch, flags) {
								Some(true) => matched = true,
								Some(false) => {}
								None => return Outcome::AbortAll,
							}
							p_ch = 0;
						}
					} else if t_ch == p_ch {
						matched = true;
					}
					prev_ch = p_ch;
					p += 1;
					p_ch = at(pattern, p);
					if p_ch == b']' {
						break;
					}
				}
				if matched == negated || (flags.pathname && t_ch == b'/') {
					return Outcome::NoMatch;
				}
			}
			b'\\' => {
				// A literal match with the following character, which is
				// compared as written, without case folding.
				p += 1;
				if t_ch != at(pattern, p) {
					return Outcome::NoMatch;
				}
			}
			_ => {
				if t_ch != p_ch {
					return Outcome::NoMatch;
				}
			}
		}
		p += 1;
		t += 1;
	}
	if at(text, t) == 0 {
		Outcome::Match
	} else {
		Outcome::NoMatch
	}
}

/// Whether `byte` belongs to the POSIX class `name`, as git's `ctype`
/// defines it, or `None` for an unknown class.
fn class_matches(name: &[u8], byte: u8, flags: Flags) -> Option<bool> {
	Some(match name {
		b"alnum" => byte.is_ascii_alphanumeric(),
		b"alpha" => byte.is_ascii_alphabetic(),
		b"blank" => matches!(byte, b' ' | b'\t'),
		b"cntrl" => byte.is_ascii_control(),
		b"digit" => byte.is_ascii_digit(),
		b"graph" => byte.is_ascii_graphic(),
		b"lower" => byte.is_ascii_lowercase(),
		b"print" => byte.is_ascii_graphic() || byte == b' ',
		b"punct" => byte.is_ascii_punctuation(),
		b"space" => matches!(byte, b' ' | b'\t' | b'\n' | b'\r'),
		b"upper" => {
			byte.is_ascii_uppercase() || (flags.casefold && byte.is_ascii_lowercase())
		}
		b"xdigit" => byte.is_ascii_hexdigit(),
		_ => return None,
	})
}

#[cfg(test)]
mod tests {
	use super::*;

	const PATH: Flags = Flags {
		pathname: true,
		casefold: false,
	};
	const BASE: Flags = Flags {
		pathname: false,
		casefold: false,
	};

	fn check(pattern: &str, text: &str, flags: Flags) -> bool {
		wildmatch(pattern.as_bytes(), text.as_bytes(), flags)
	}

	#[test]
	fn matches_literals_and_wildcards() {
		assert!(check("foo", "foo", BASE));
		assert!(!check("foo", "bar", BASE));
		assert!(check("*.dat", "x.dat", BASE));
		assert!(check("???", "abc", BASE));
		assert!(!check("??", "abc", BASE));
		assert!(check("*", "", BASE));
		assert!(check("\\*", "*", BASE));
		assert!(!check("\\*", "x", BASE));
	}

	#[test]
	fn keeps_single_stars_within_a_directory() {
		assert!(check("foo/*", "foo/bar", PATH));
		assert!(!check("foo/*", "foo/bar/baz", PATH));
		assert!(check("*/bar", "foo/bar", PATH));
		assert!(!check("*.c", "dir/a.c", PATH));
		assert!(!check("f?o/bar", "f/o/bar", PATH));
	}

	#[test]
	fn spans_directories_with_double_stars() {
		assert!(check("**/foo", "foo", PATH));
		assert!(check("**/foo", "a/b/foo", PATH));
		assert!(check("foo/**", "foo/a/b", PATH));
		assert!(check("a/**/b", "a/b", PATH));
		assert!(check("a/**/b", "a/x/y/b", PATH));
		assert!(!check("a/**/b", "a/x/c", PATH));
		assert!(check("sub/**", "sub/x.dat", PATH));
	}

	#[test]
	fn matches_bracket_expressions() {
		assert!(check("[abc]", "b", BASE));
		assert!(!check("[!abc]", "b", BASE));
		assert!(check("[^abc]", "d", BASE));
		assert!(check("[a-c]x", "bx", BASE));
		assert!(check("[[:digit:]]", "7", BASE));
		assert!(!check("[[:digit:]]", "x", BASE));
		assert!(check("[]]", "]", BASE));
		assert!(!check("a[/]b", "a/b", PATH));
	}

	#[test]
	fn folds_case_when_asked() {
		let fold = Flags {
			pathname: true,
			casefold: true,
		};
		assert!(check("*.DAT", "x.dat", fold));
		assert!(check("[A-C]", "b", fold));
		assert!(!check("*.DAT", "x.dat", PATH));
	}
}
