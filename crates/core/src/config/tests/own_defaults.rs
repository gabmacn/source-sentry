//! Keeps the project's explicit restatement of defaults in step with the core.

use super::snapshot;
use crate::config::{
	BUILT_IN_DOCUMENT_TYPES, ConfigSet, DEFAULT_CONTEXT, DEFAULT_TIME_ZONE,
	IMPLICIT_VERSION,
};
use crate::evaluate::settings::EffectiveSettings;
use crate::model::RepoPath;

/// Reads from the crate-relative product root in both repository layouts.
#[test]
fn own_project_restates_the_built_in_defaults() {
	let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
		.join("../../.source-sentry/source-sentry.yaml");
	let text = std::fs::read_to_string(file).unwrap();
	let configs =
		ConfigSet::load(&snapshot(&[(".source-sentry/source-sentry.yaml", &text)]))
			.unwrap();
	assert_eq!(configs.version().value, IMPLICIT_VERSION);
	assert_eq!(configs.time_zone().value, DEFAULT_TIME_ZONE);
	assert_eq!(
		configs.contexts()[DEFAULT_CONTEXT].context,
		crate::policy::default_context(IMPLICIT_VERSION).unwrap()
	);
	let settings = configs.settings(&RepoPath::ROOT);
	let count = BUILT_IN_DOCUMENT_TYPES.len();
	assert_eq!(
		settings.include.value[..count]
			.iter()
			.map(crate::config::PathPattern::as_str)
			.collect::<Vec<_>>(),
		BUILT_IN_DOCUMENT_TYPES
			.iter()
			.map(|(pattern, _)| *pattern)
			.collect::<Vec<_>>()
	);
	// The first three explicit mappings follow the three built-in mappings.
	let explicit = &settings.adapters[count..count * 2];
	assert_eq!(
		explicit
			.iter()
			.map(|mapping| (mapping.value.pattern.as_str(), mapping.value.adapter))
			.collect::<Vec<_>>(),
		BUILT_IN_DOCUMENT_TYPES
	);
	assert_eq!(
		EffectiveSettings::new(&settings.defaults, None).grace.value,
		0
	);
	// Product-specific example patterns, adapters and ignores are deliberate
	// additions and are outside this comparison.
}
