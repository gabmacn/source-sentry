//! The strict YAML options shared by every parser of frontmatter, configs and
//! presets.

use std::collections::BTreeMap;
use std::fmt;

use serde::Deserialize;
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde_saphyr::{DuplicateKeyPolicy, MergeKeyPolicy, Spanned};

/// The strict parser options: merge keys, duplicate keys, unsupported tags and
/// YAML 1.1 booleans are rejected. Callers use the single-document entry
/// points, which reject a second document. Error snippets are off, because
/// every caller renders its own diagnostics.
pub(crate) fn strict_options() -> serde_saphyr::Options {
	serde_saphyr::options! {
		strict_booleans: true,
		reject_unsupported_tags: true,
		merge_keys: MergeKeyPolicy::Error,
		duplicate_keys: DuplicateKeyPolicy::Error,
		with_snippet: false,
	}
}

/// Where each field of the YAML document `text` starts, by its path: a
/// mapping key appends `.key`, and a sequence item `[index]`, so the paths
/// read like diagnostics' fields, such as `source-sentry.classes[1]` or
/// `schedule.cadence`. A key's line is the key's own, and an item's is the
/// item's. Keys that are not strings are left out, with what lies below
/// them. Each position is a line and a column, counted from 1. `None`
/// when `text` does not parse.
pub(crate) fn field_lines(text: &str) -> Option<BTreeMap<String, (u64, u64)>> {
	field_lines_with(text, |path, key| {
		if path.is_empty() {
			key.to_owned()
		} else {
			format!("{path}.{key}")
		}
	})
}

/// Like [`field_lines`], with mapping paths built by `key_path` from the
/// parent's path and the decoded key. Sequence items still append `[index]`.
/// This lets lockfiles quote entry keys without splitting paths that may
/// themselves contain dots, brackets or quotes. Parses `text` once.
pub(crate) fn field_lines_with(
	text: &str,
	key_path: impl Fn(&str, &str) -> String,
) -> Option<BTreeMap<String, (u64, u64)>> {
	let tree = serde_saphyr::from_str::<Tree>(text).ok()?;
	let mut lines = BTreeMap::new();
	tree.walk("", &key_path, &mut lines);
	Some(lines)
}

/// A YAML value with the position of every key and item below it.
enum Tree {
	/// A scalar, or anything without fields.
	Leaf,
	/// A sequence's items.
	Seq(Vec<Spanned<Tree>>),
	/// A mapping's entries, with string keys only.
	Map(Vec<(Spanned<Key>, Tree)>),
}

impl Tree {
	/// Adds the line of everything below `self`, whose path is `path`.
	fn walk(
		&self,
		path: &str,
		key_path: &impl Fn(&str, &str) -> String,
		lines: &mut BTreeMap<String, (u64, u64)>,
	) {
		match self {
			Self::Leaf => {}
			Self::Seq(items) => {
				for (index, item) in items.iter().enumerate() {
					let path = format!("{path}[{index}]");
					lines.insert(
						path.clone(),
						(item.referenced.line(), item.referenced.column()),
					);
					item.value.walk(&path, key_path, lines);
				}
			}
			Self::Map(entries) => {
				for (key, value) in entries {
					let Key(Some(name)) = &key.value else {
						continue;
					};
					let path = key_path(path, name);
					lines.insert(
						path.clone(),
						(key.referenced.line(), key.referenced.column()),
					);
					value.walk(&path, key_path, lines);
				}
			}
		}
	}
}

impl<'de> Deserialize<'de> for Tree {
	fn deserialize<D: de::Deserializer<'de>>(
		deserializer: D,
	) -> Result<Self, D::Error> {
		deserializer.deserialize_any(TreeVisitor)
	}
}

/// Builds a [`Tree`].
struct TreeVisitor;

impl<'de> Visitor<'de> for TreeVisitor {
	type Value = Tree;

	fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		formatter.write_str("any YAML value")
	}

	fn visit_bool<E: de::Error>(self, _: bool) -> Result<Tree, E> {
		Ok(Tree::Leaf)
	}

	fn visit_i64<E: de::Error>(self, _: i64) -> Result<Tree, E> {
		Ok(Tree::Leaf)
	}

	fn visit_i128<E: de::Error>(self, _: i128) -> Result<Tree, E> {
		Ok(Tree::Leaf)
	}

	fn visit_u64<E: de::Error>(self, _: u64) -> Result<Tree, E> {
		Ok(Tree::Leaf)
	}

	fn visit_u128<E: de::Error>(self, _: u128) -> Result<Tree, E> {
		Ok(Tree::Leaf)
	}

	fn visit_f64<E: de::Error>(self, _: f64) -> Result<Tree, E> {
		Ok(Tree::Leaf)
	}

	fn visit_str<E: de::Error>(self, _: &str) -> Result<Tree, E> {
		Ok(Tree::Leaf)
	}

	fn visit_unit<E: de::Error>(self) -> Result<Tree, E> {
		Ok(Tree::Leaf)
	}

	fn visit_none<E: de::Error>(self) -> Result<Tree, E> {
		Ok(Tree::Leaf)
	}

	fn visit_some<D: de::Deserializer<'de>>(
		self,
		deserializer: D,
	) -> Result<Tree, D::Error> {
		Tree::deserialize(deserializer)
	}

	fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Tree, A::Error> {
		let mut items = Vec::new();
		while let Some(item) = seq.next_element::<Spanned<Tree>>()? {
			items.push(item);
		}
		Ok(Tree::Seq(items))
	}

	fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Tree, A::Error> {
		let mut entries = Vec::new();
		while let Some(key) = map.next_key::<Spanned<Key>>()? {
			entries.push((key, map.next_value::<Tree>()?));
		}
		Ok(Tree::Map(entries))
	}
}

/// A mapping key, as text when it is a string.
struct Key(Option<String>);

impl<'de> Deserialize<'de> for Key {
	fn deserialize<D: de::Deserializer<'de>>(
		deserializer: D,
	) -> Result<Self, D::Error> {
		deserializer.deserialize_any(KeyVisitor)
	}
}

/// Reads a key: a string's text, and any other key consumed as nothing.
struct KeyVisitor;

impl<'de> Visitor<'de> for KeyVisitor {
	type Value = Key;

	fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		formatter.write_str("a mapping key")
	}

	fn visit_str<E: de::Error>(self, value: &str) -> Result<Key, E> {
		Ok(Key(Some(value.to_owned())))
	}

	fn visit_string<E: de::Error>(self, value: String) -> Result<Key, E> {
		Ok(Key(Some(value)))
	}

	fn visit_bool<E: de::Error>(self, _: bool) -> Result<Key, E> {
		Ok(Key(None))
	}

	fn visit_i64<E: de::Error>(self, _: i64) -> Result<Key, E> {
		Ok(Key(None))
	}

	fn visit_u64<E: de::Error>(self, _: u64) -> Result<Key, E> {
		Ok(Key(None))
	}

	fn visit_f64<E: de::Error>(self, _: f64) -> Result<Key, E> {
		Ok(Key(None))
	}

	fn visit_unit<E: de::Error>(self) -> Result<Key, E> {
		Ok(Key(None))
	}

	fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Key, A::Error> {
		while seq.next_element::<de::IgnoredAny>()?.is_some() {}
		Ok(Key(None))
	}

	fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Key, A::Error> {
		while map
			.next_entry::<de::IgnoredAny, de::IgnoredAny>()?
			.is_some()
		{}
		Ok(Key(None))
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn fields_are_found_on_their_lines() {
		let lines = field_lines(
			"source-sentry:\n  classes:\n    - alpha\n    - missing\n  sources:\n    \
			 - path: /a.rs\n? [complex]\n: ignored\nschedule:\n  grace: 5\n  cadence:\n    \
			 dtstart: 2026-01-01\n",
		)
		.unwrap();
		assert_eq!(lines["source-sentry"], (1, 1));
		assert_eq!(lines["source-sentry.classes"], (2, 3));
		assert_eq!(lines["source-sentry.classes[1]"].0, 4);
		assert_eq!(lines["source-sentry.sources[0].path"].0, 6);
		assert_eq!(lines["schedule.cadence"], (11, 3));
		assert_eq!(lines["schedule.cadence.dtstart"].0, 12);
		assert!(!lines.keys().any(|key| key.contains("complex")));
	}

	#[test]
	fn json_keys_and_items_have_their_own_spans() {
		let lines = field_lines(
			r#"{
  "documents": {
    "docs/a.md": {
      "classes": [
        "alpha",
        "missing"
      ],
      "sources": [
        {
          "path": "/a.rs"
        }
      ]
    }
  }
}"#,
		)
		.unwrap();
		assert_eq!(lines["documents"], (2, 3));
		assert_eq!(lines["documents.docs/a.md"], (3, 5));
		assert_eq!(lines["documents.docs/a.md.classes[1]"], (6, 9));
		assert_eq!(lines["documents.docs/a.md.sources[0]"], (9, 9));
		assert_eq!(lines["documents.docs/a.md.sources[0].path"], (10, 11));
	}
}
