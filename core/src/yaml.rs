//! The strict YAML options shared by every parser of frontmatter, configs and
//! presets.

use serde_saphyr::{DuplicateKeyPolicy, MergeKeyPolicy};

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
