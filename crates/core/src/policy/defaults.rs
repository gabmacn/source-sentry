//! The built-in default context, one YAML file per default-policy version.
//!
//! The files live in `crates/core/defaults/` and are compiled in, so each one
//! also documents the defaults it defines.

use serde_saphyr::Options;

use crate::model::Context;

use super::PolicyError;

/// The default-policy versions this build ships, oldest first. The repository
/// config's `version` pins one of them.
pub const DEFAULTS_VERSIONS: [u32; 1] = [1];

/// Version 1: passes `fresh`, reports `missed` as a notice and warns on every
/// other fact.
const CONTEXT_V1: &str = include_str!("../../defaults/context-v1.yaml");

/// The YAML source of the built-in default context for a default-policy
/// version, comments included, or `None` when this build does not ship that
/// version.
#[must_use]
pub fn default_context_source(version: u32) -> Option<&'static str> {
	match version {
		1 => Some(CONTEXT_V1),
		_ => None,
	}
}

/// The built-in default context for a default-policy version. It applies only
/// when the repository config declares no `default` context; see
/// [`select_context`](super::select_context).
///
/// # Errors
///
/// - [`PolicyError::UnsupportedDefaultsVersion`] when this build does not
///   ship `version`.
/// - [`PolicyError::BrokenDefaults`] when the shipped file does not parse,
///   which the crate's tests rule out for every shipped version.
pub fn default_context(version: u32) -> Result<Context, PolicyError> {
	let source = default_context_source(version)
		.ok_or(PolicyError::UnsupportedDefaultsVersion { version })?;
	parse_context(source).map_err(|error| PolicyError::BrokenDefaults {
		version,
		reason: error.to_string(),
	})
}

/// Parses one context with the strict YAML options.
pub(super) fn parse_context(
	source: &str,
) -> Result<Context, serde_saphyr::Error> {
	serde_saphyr::from_str_with_options(source, strict_options())
}

/// The strict YAML parser options. `from_str_with_options` already rejects a
/// file with more than one document.
fn strict_options() -> Options {
	crate::yaml::strict_options()
}
