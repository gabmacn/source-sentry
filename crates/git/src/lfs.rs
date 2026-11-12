//! Native Git LFS pointer handling.
//!
//! The LFS clean filter never runs, because it writes into
//! `.git/lfs/objects`. Instead this module decides what the filter would hand
//! to git, following the pointer specification
//! (<https://github.com/git-lfs/git-lfs/blob/main/docs/spec.md>) and the
//! reference implementation's clean filter (`lfs/gitfilter_clean.go` and
//! `lfs/pointer.go` in git-lfs 3):
//!
//! - Content shorter than 1024 bytes that the reference decoder accepts as a
//!   pointer is passed through unchanged, byte for byte. That includes the
//!   empty file, which "is the pointer for an empty file".
//! - A pointer with `ext-<n>-<name>` lines is rejected.
//! - Anything else is hydrated content, which becomes the canonical pointer
//!   `version https://git-lfs.github.com/spec/v1\noid sha256:<hex>\nsize
//!   <n>\n`.

use std::{
	fs::File,
	io::{self, Read as _},
	path::Path,
};

use sha2::{Digest as _, Sha256};

use crate::ids::to_hex;

/// The version line that the reference implementation writes.
const LATEST_VERSION: &[u8] = b"https://git-lfs.github.com/spec/v1";
/// Every version the reference decoder accepts, including the alpha and
/// pre-release URLs.
const VERSIONS: [&[u8]; 3] = [
	b"http://git-media.io/v/2",
	b"https://hawser.github.com/spec/v1",
	LATEST_VERSION,
];
/// The reference implementation's `BlobSizeCutoff`: content this long or
/// longer is never treated as a pointer.
const POINTER_CUTOFF: usize = 1024;
/// The keys of a pointer, in the order they must appear.
const KEYS: [&[u8]; 3] = [b"version", b"oid", b"size"];

/// What the LFS clean filter would do with a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Cleaned {
	/// The file already is a pointer, or is empty, and passes through as it
	/// is.
	PassThrough,
	/// The file is a pointer that uses extensions.
	Extension,
	/// The file holds content, which the filter replaces with this pointer.
	Hydrated(Vec<u8>),
}

/// Reads the file at `path` and decides what the LFS clean filter would do
/// with it. Hydrated content is hashed as it streams, so large files are never
/// held in memory.
///
/// # Errors
///
/// Fails when the file cannot be read.
pub(crate) fn clean_file(path: &Path) -> io::Result<Cleaned> {
	let mut file = File::open(path)?;
	let mut head = Vec::with_capacity(POINTER_CUTOFF);
	(&mut file)
		.take(u64::try_from(POINTER_CUTOFF).unwrap_or(u64::MAX))
		.read_to_end(&mut head)?;
	if head.len() < POINTER_CUTOFF {
		match decode(&head) {
			Some(Pointer { extensions: false }) => return Ok(Cleaned::PassThrough),
			Some(Pointer { extensions: true }) => return Ok(Cleaned::Extension),
			None => {}
		}
	}
	let mut hasher = Sha256::new();
	hasher.update(&head);
	let mut size = u64::try_from(head.len()).unwrap_or(u64::MAX);
	let mut buffer = vec![0; 64 * 1024];
	loop {
		let read = match file.read(&mut buffer) {
			Ok(0) => break,
			Ok(read) => read,
			Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
			Err(error) => return Err(error),
		};
		hasher.update(&buffer[..read]);
		size = size.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
	}
	Ok(Cleaned::Hydrated(pointer(
		&to_hex(&hasher.finalize()),
		size,
	)))
}

/// The canonical pointer for content with the given SHA-256 digest and size.
pub(crate) fn pointer(sha256_hex: &str, size: u64) -> Vec<u8> {
	let mut text = Vec::with_capacity(160);
	text.extend_from_slice(b"version ");
	text.extend_from_slice(LATEST_VERSION);
	text.extend_from_slice(
		format!("\noid sha256:{sha256_hex}\nsize {size}\n").as_bytes(),
	);
	text
}

/// A pointer that the reference decoder accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Pointer {
	/// Whether it has `ext-<n>-<name>` lines.
	extensions: bool,
}

/// Decodes `content` exactly as the reference implementation's `DecodeFrom`
/// does, or returns `None` where it would report "not a pointer".
fn decode(content: &[u8]) -> Option<Pointer> {
	if content.is_empty() {
		return Some(Pointer { extensions: false });
	}
	let data = trim_space(content);
	if !(contains(data, b"git-media")
		|| contains(data, b"hawser")
		|| contains(data, b"git-lfs"))
	{
		return None;
	}

	let mut values: [Option<&[u8]>; 3] = [None; 3];
	let mut extensions: Vec<(&[u8], &[u8])> = Vec::new();
	let mut line = 0;
	for text in scan_lines(data) {
		if text.is_empty() {
			continue;
		}
		let space = text.iter().position(|&byte| byte == b' ')?;
		let (key, value) = (&text[..space], &text[space + 1..]);
		let expected = KEYS.get(line)?;
		if key != *expected {
			if !is_extension_key(key) {
				return None;
			}
			// Later lines replace earlier ones with the same key, as in a Go
			// map.
			extensions.retain(|(existing, _)| *existing != key);
			extensions.push((key, value));
			continue;
		}
		values[line] = Some(value);
		line += 1;
	}

	let [version, oid, size] = values;
	if !VERSIONS.contains(&version?) {
		return None;
	}
	if !is_sha256_oid(oid?) {
		return None;
	}
	parse_size(size.unwrap_or_default())?;
	let mut priorities = Vec::with_capacity(extensions.len());
	for (key, value) in &extensions {
		if !is_sha256_oid(value) {
			return None;
		}
		// `is_extension_key` guarantees `ext-<digit>-`.
		let priority = key.get(4)?;
		if priorities.contains(priority) {
			return None;
		}
		priorities.push(*priority);
	}
	Some(Pointer {
		extensions: !extensions.is_empty(),
	})
}

/// Go's `bytes.TrimSpace`: removes leading and trailing Unicode white space.
fn trim_space(mut data: &[u8]) -> &[u8] {
	while let Some((character, length)) = first_char(data)
		&& character.is_whitespace()
	{
		data = &data[length..];
	}
	while let Some((character, length)) = last_char(data)
		&& character.is_whitespace()
	{
		data = &data[..data.len() - length];
	}
	data
}

/// The first UTF-8 character of `data` and its length, if it is valid.
fn first_char(data: &[u8]) -> Option<(char, usize)> {
	(1..=data.len().min(4)).find_map(|length| {
		std::str::from_utf8(&data[..length])
			.ok()
			.and_then(|text| text.chars().next().map(|character| (character, length)))
	})
}

/// The last UTF-8 character of `data` and its length, if it is valid.
fn last_char(data: &[u8]) -> Option<(char, usize)> {
	(1..=data.len().min(4)).find_map(|length| {
		std::str::from_utf8(&data[data.len() - length..])
			.ok()
			.and_then(|text| text.chars().next().map(|character| (character, length)))
	})
}

/// Go's `bufio.ScanLines`: lines split at `\n`, each without one trailing
/// `\r`, and a final line without a newline.
fn scan_lines(data: &[u8]) -> impl Iterator<Item = &[u8]> {
	let ends_with_newline = data.last() == Some(&b'\n');
	let mut lines: Vec<&[u8]> = data.split(|&byte| byte == b'\n').collect();
	if ends_with_newline {
		lines.pop();
	}
	lines
		.into_iter()
		.map(|line| line.strip_suffix(b"\r").unwrap_or(line))
}

/// Whether `haystack` contains `needle`.
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
	haystack
		.windows(needle.len())
		.any(|window| window == needle)
}

/// The reference implementation's `\Aext-\d{1}-\w+` key pattern, which only
/// anchors at the start.
fn is_extension_key(key: &[u8]) -> bool {
	key.len() > 6
		&& key.starts_with(b"ext-")
		&& key[4].is_ascii_digit()
		&& key[5] == b'-'
		&& (key[6].is_ascii_alphanumeric() || key[6] == b'_')
}

/// Whether `value` is `sha256:` followed by 64 lowercase hexadecimal digits.
fn is_sha256_oid(value: &[u8]) -> bool {
	value.strip_prefix(b"sha256:").is_some_and(|hex| {
		hex.len() == 64
			&& hex
				.iter()
				.all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
	})
}

/// Go's `strconv.ParseInt(value, 10, 64)` with the reference implementation's
/// check that the size is not negative.
fn parse_size(value: &[u8]) -> Option<u64> {
	let text = std::str::from_utf8(value).ok()?;
	let size: i64 = text.parse().ok()?;
	u64::try_from(size).ok()
}

#[cfg(test)]
mod tests {
	use super::*;

	const HEX: &str =
		"4d7a214614ab2935c943f9e0ff69d22eadbb8f32b1258daaa5e2ca24d17e2393";

	fn canonical() -> String {
		format!(
			"version https://git-lfs.github.com/spec/v1\noid sha256:{HEX}\nsize 12345\n"
		)
	}

	#[test]
	fn generates_the_specification_example() {
		assert_eq!(pointer(HEX, 12_345), canonical().into_bytes());
	}

	#[test]
	fn accepts_canonical_and_empty_pointers() {
		assert_eq!(
			decode(canonical().as_bytes()),
			Some(Pointer { extensions: false })
		);
		assert_eq!(decode(b""), Some(Pointer { extensions: false }));
	}

	#[test]
	fn accepts_what_the_reference_decoder_accepts() {
		let crlf = canonical().replace('\n', "\r\n");
		assert!(decode(crlf.as_bytes()).is_some());
		let padded = format!("\n  {}\n\n", canonical());
		assert!(decode(padded.as_bytes()).is_some());
		let hawser = canonical().replace("git-lfs.github.com", "hawser.github.com");
		assert!(decode(hawser.as_bytes()).is_some());
		let no_newline = canonical().trim_end().to_owned();
		assert!(decode(no_newline.as_bytes()).is_some());
		let signed = canonical().replace("size 12345", "size +12345");
		assert!(decode(signed.as_bytes()).is_some());
	}

	#[test]
	fn rejects_what_the_reference_decoder_rejects() {
		for text in [
			canonical().replace("sha256:4d", "sha256:4D"),
			canonical().replace("size 12345", "size -1"),
			canonical().replace(HEX, &format!("{HEX} ")),
			canonical().replace(
				"version https://git-lfs.github.com/spec/v1",
				"version https://git-lfs.github.com/spec/v2",
			),
			format!("{}extra line\n", canonical()),
			canonical().replace("oid", "uid"),
			"hello world\n".to_owned(),
			"version x\n".to_owned(),
		] {
			assert_eq!(decode(text.as_bytes()), None, "{text:?}");
		}
	}

	#[test]
	fn detects_extensions() {
		let with_extension = format!(
			"version https://git-lfs.github.com/spec/v1\next-0-foo sha256:{HEX}\noid sha256:{HEX}\nsize 12345\n"
		);
		assert_eq!(
			decode(with_extension.as_bytes()),
			Some(Pointer { extensions: true })
		);
		let duplicate = format!(
			"version https://git-lfs.github.com/spec/v1\next-0-foo sha256:{HEX}\next-0-bar sha256:{HEX}\noid sha256:{HEX}\nsize 1\n"
		);
		assert_eq!(decode(duplicate.as_bytes()), None);
		let bad_oid = format!(
			"version https://git-lfs.github.com/spec/v1\next-0-foo md5:00\noid sha256:{HEX}\nsize 1\n"
		);
		assert_eq!(decode(bad_oid.as_bytes()), None);
	}
}
