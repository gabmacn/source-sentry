//! Selecting the context for a run.

use std::collections::BTreeMap;

use crate::model::Context;

use super::{PolicyError, default_context};

/// The name of the default context: the one that applies when the caller
/// names none. A repository context with this name replaces the built-in
/// default context.
pub const DEFAULT_CONTEXT_NAME: &str = "default";

/// Where a selected context comes from, for the report's origins.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ContextOrigin {
	/// The built-in default context of this default-policy version.
	BuiltIn {
		/// The default-policy version, pinned by the repository config.
		version: u32,
	},
	/// Declared under `define.contexts` in the repository config.
	Repository,
}

/// The context a run uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectedContext {
	/// The context's name. Directory configs add scoped rules to the context
	/// of this name.
	pub name: String,
	/// Where the context comes from.
	pub origin: ContextOrigin,
	/// The context itself.
	pub context: Context,
}

/// Selects the context for a run.
///
/// # Parameters
///
/// - `requested`: the context the caller named, or `None` for the default.
/// - `declared`: the contexts under `define.contexts` in the repository
///   config, by name.
/// - `defaults_version`: the default-policy version that the repository
///   config pins.
///
/// Without a name, or with the name [`DEFAULT_CONTEXT_NAME`], the result is
/// the repository's `default` context when it declares one, which replaces the
/// built-in default entirely. Otherwise it is the built-in default
/// context for `defaults_version`. Any other name must be declared.
///
/// Selection does not validate the context; [`validate_context`] does.
///
/// [`validate_context`]: super::validate_context
///
/// # Errors
///
/// - [`PolicyError::UnsupportedDefaultsVersion`] when this build does not ship
///   `defaults_version`, whichever context is selected.
/// - [`PolicyError::UnknownContext`] when `requested` names a context that is
///   not declared. It never falls back to the default context.
/// - [`PolicyError::BrokenDefaults`] when the built-in file does not parse.
pub fn select_context(
	requested: Option<&str>,
	declared: &BTreeMap<String, Context>,
	defaults_version: u32,
) -> Result<SelectedContext, PolicyError> {
	// Load the built-in context first, so that an unsupported version is
	// rejected even when the repository's own context is selected.
	let built_in = default_context(defaults_version)?;
	let name = requested.unwrap_or(DEFAULT_CONTEXT_NAME);
	if let Some(context) = declared.get(name) {
		return Ok(SelectedContext {
			name: name.to_owned(),
			origin: ContextOrigin::Repository,
			context: context.clone(),
		});
	}
	if name == DEFAULT_CONTEXT_NAME {
		return Ok(SelectedContext {
			name: name.to_owned(),
			origin: ContextOrigin::BuiltIn {
				version: defaults_version,
			},
			context: built_in,
		});
	}
	Err(PolicyError::UnknownContext {
		name: name.to_owned(),
		available: available_names(declared),
	})
}

/// Every selectable name, sorted: the declared contexts and `default`.
fn available_names(declared: &BTreeMap<String, Context>) -> Vec<String> {
	let mut names: Vec<String> = declared.keys().cloned().collect();
	if !declared.contains_key(DEFAULT_CONTEXT_NAME) {
		names.push(DEFAULT_CONTEXT_NAME.to_owned());
		names.sort();
	}
	names
}
