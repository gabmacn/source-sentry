//! A YAML parse of the whole frontmatter, which checks the line scan.
//!
//! The parse is lenient: it only needs the top-level keys, where each starts,
//! and every other entry's value with aliases resolved. When it fails, for
//! example on invalid YAML in another tool's key, the scan stands alone, so
//! that other keys' values still cannot make reading fail.

use serde::Deserialize;
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde_saphyr::Spanned;

use super::{KEY, scan};

/// What the parse found, when the frontmatter is a YAML mapping or empty.
#[derive(Debug, PartialEq)]
pub(super) struct Whole {
	/// The file lines, counting the opening `---` as line 1, of the
	/// top-level keys that decode to `source-sentry`.
	pub(super) keys: Vec<usize>,
	/// Every other top-level entry, in order, with aliases resolved.
	pub(super) others: Vec<(Node, Node)>,
}

/// Any YAML value, with aliases resolved, compared by value.
#[derive(Debug, PartialEq)]
pub(super) enum Node {
	/// `null` or an empty value.
	Null,
	/// A boolean.
	Bool(bool),
	/// An integer.
	Int(i128),
	/// A float, by its bits, so that equal floats compare equal.
	Float(u64),
	/// A string.
	Str(String),
	/// A sequence.
	Seq(Vec<Node>),
	/// A mapping, in order, keys included whatever their kind.
	Map(Vec<(Node, Node)>),
}

impl<'de> Deserialize<'de> for Node {
	fn deserialize<D: de::Deserializer<'de>>(
		deserializer: D,
	) -> Result<Self, D::Error> {
		deserializer.deserialize_any(NodeVisitor)
	}
}

/// Builds a [`Node`] from whatever the parser holds.
struct NodeVisitor;

impl<'de> Visitor<'de> for NodeVisitor {
	type Value = Node;

	fn expecting(
		&self,
		formatter: &mut std::fmt::Formatter<'_>,
	) -> std::fmt::Result {
		formatter.write_str("any YAML value")
	}

	fn visit_unit<E: de::Error>(self) -> Result<Node, E> {
		Ok(Node::Null)
	}

	fn visit_none<E: de::Error>(self) -> Result<Node, E> {
		Ok(Node::Null)
	}

	fn visit_some<D: de::Deserializer<'de>>(
		self,
		deserializer: D,
	) -> Result<Node, D::Error> {
		Node::deserialize(deserializer)
	}

	fn visit_bool<E: de::Error>(self, value: bool) -> Result<Node, E> {
		Ok(Node::Bool(value))
	}

	fn visit_i64<E: de::Error>(self, value: i64) -> Result<Node, E> {
		Ok(Node::Int(i128::from(value)))
	}

	fn visit_i128<E: de::Error>(self, value: i128) -> Result<Node, E> {
		Ok(Node::Int(value))
	}

	fn visit_u64<E: de::Error>(self, value: u64) -> Result<Node, E> {
		Ok(Node::Int(i128::from(value)))
	}

	fn visit_u128<E: de::Error>(self, value: u128) -> Result<Node, E> {
		i128::try_from(value)
			.map(Node::Int)
			.map_err(|_| E::custom("integer out of range"))
	}

	fn visit_f64<E: de::Error>(self, value: f64) -> Result<Node, E> {
		Ok(Node::Float(value.to_bits()))
	}

	fn visit_str<E: de::Error>(self, value: &str) -> Result<Node, E> {
		Ok(Node::Str(value.to_owned()))
	}

	fn visit_string<E: de::Error>(self, value: String) -> Result<Node, E> {
		Ok(Node::Str(value))
	}

	fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Node, A::Error> {
		let mut items = Vec::new();
		while let Some(item) = seq.next_element::<Node>()? {
			items.push(item);
		}
		Ok(Node::Seq(items))
	}

	fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Node, A::Error> {
		let mut entries = Vec::new();
		while let Some(key) = map.next_key::<Node>()? {
			entries.push((key, map.next_value::<Node>()?));
		}
		Ok(Node::Map(entries))
	}
}

impl<'de> Deserialize<'de> for Whole {
	fn deserialize<D: de::Deserializer<'de>>(
		deserializer: D,
	) -> Result<Self, D::Error> {
		deserializer.deserialize_any(WholeVisitor)
	}
}

/// Splits the top-level mapping into the declaration's keys and the rest.
struct WholeVisitor;

impl<'de> Visitor<'de> for WholeVisitor {
	type Value = Whole;

	fn expecting(
		&self,
		formatter: &mut std::fmt::Formatter<'_>,
	) -> std::fmt::Result {
		formatter.write_str("a mapping")
	}

	fn visit_unit<E: de::Error>(self) -> Result<Whole, E> {
		Ok(Whole {
			keys: Vec::new(),
			others: Vec::new(),
		})
	}

	fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Whole, A::Error> {
		let mut keys = Vec::new();
		let mut others = Vec::new();
		while let Some(key) = map.next_key::<Spanned<Node>>()? {
			let value = map.next_value::<Node>()?;
			if key.value == Node::Str(KEY.to_owned()) {
				// The parsed text starts on the line after the opening `---`.
				let line = usize::try_from(key.referenced.line())
					.map_err(|_| de::Error::custom("line out of range"))?;
				keys.push(line + 1);
			} else {
				others.push((key.value, value));
			}
		}
		Ok(Whole { keys, others })
	}
}

/// The whole frontmatter of `bytes`, parsed, or `None` when there is no
/// frontmatter, or it is not UTF-8, or it is not a YAML mapping.
pub(super) fn parse(bytes: &[u8]) -> Option<Whole> {
	let range = scan::frontmatter(bytes)?;
	let text = std::str::from_utf8(&bytes[range]).ok()?;
	// Frontmatter without content, such as what removing the only key
	// leaves, is an empty mapping.
	if text.lines().all(|line| {
		let line = line.trim();
		line.is_empty() || line.starts_with('#')
	}) {
		return Some(Whole {
			keys: Vec::new(),
			others: Vec::new(),
		});
	}
	serde_saphyr::from_str::<Whole>(text).ok()
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn keys_are_found_by_line_and_others_resolved() {
		let whole = parse(
			b"---\nbase: &a 1\n? [one, two]\n: x\ntitle: \"hello\n  source-sentry: {}\n  world\"\n\
			  source-sentry:\n  owners: [b]\nother: *a\n---\n",
		)
		.unwrap();
		assert_eq!(whole.keys, [8]);
		assert_eq!(whole.others.len(), 4);
		assert_eq!(
			whole.others[3],
			(Node::Str("other".to_owned()), Node::Int(1))
		);
	}

	#[test]
	fn what_is_not_a_mapping_is_not_parsed() {
		assert_eq!(parse(b"---\n- a\n---\n"), None);
		assert_eq!(parse(b"no frontmatter\n"), None);
		assert_eq!(parse(b"---\ntitle: [unclosed\n---\n"), None);
		assert_eq!(
			parse(b"---\n# only a comment\n---\n"),
			Some(Whole {
				keys: Vec::new(),
				others: Vec::new()
			})
		);
	}
}
