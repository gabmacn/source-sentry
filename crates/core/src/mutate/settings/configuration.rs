//! Value comparisons for project policy and effective directory configuration.

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::Value;

use super::super::{MutateError, Placement};
use crate::config::{ConfigSet, Origin, PathPattern, Scope};
use crate::model::{DocumentId, RepoPath};
use crate::ports::{BackendError, BackendErrorKind};

/// One effective configuration value changed by a move.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigurationChange {
	/// The setting's public name.
	pub setting: &'static str,
	/// Its former value, without origins.
	pub before: String,
	/// Its destination value, without origins.
	pub after: String,
}

/// Compares values that affect interpretation, eligibility, policy and series.
pub(in crate::mutate) fn changes(
	before: (&ConfigSet, &DocumentId, &Placement),
	after: (&ConfigSet, &DocumentId, &Placement),
) -> Result<Vec<ConfigurationChange>, MutateError> {
	let before = values(before.0, before.1, before.2)?;
	let after = values(after.0, after.1, after.2)?;
	Ok(
		before
			.into_iter()
			.filter_map(|(setting, before)| {
				let after = after.get(setting)?;
				(before != *after).then(|| ConfigurationChange {
					setting,
					before: display(&before),
					after: display(after),
				})
			})
			.collect(),
	)
}

/// Keeps scalar strings readable while retaining complete structured values.
fn display(value: &Value) -> String {
	value
		.as_str()
		.map_or_else(|| value.to_string(), str::to_owned)
}

/// Converts validated configuration shapes for value comparison.
fn json<T: Serialize>(value: &T) -> Result<Value, MutateError> {
	serde_json::to_value(value).map_err(|error| failure(&error))
}

/// Reports an inability to build a complete configuration preview.
fn failure(error: &dyn std::fmt::Display) -> MutateError {
	MutateError::Backend(BackendError::new(
		BackendErrorKind::Io,
		format!("cannot preview project settings: {error}"),
	))
}

/// A directory relative to its project, including built-in pattern anchors.
fn anchor(
	configs: &ConfigSet,
	pattern: &PathPattern,
	origin: &Origin,
) -> String {
	if origin.scope == Scope::BuiltIn {
		return ".".to_owned();
	}
	pattern
		.anchor()
		.strip_prefix(configs.project())
		.map_or_else(
			|| pattern.anchor().to_string(),
			|path| {
				if path.is_empty() {
					".".to_owned()
				} else {
					path.to_owned()
				}
			},
		)
}

/// Normalizes default policy and drops origins before comparing configuration.
fn values(
	configs: &ConfigSet,
	document: &DocumentId,
	placement: &Placement,
) -> Result<BTreeMap<&'static str, Value>, MutateError> {
	let mut values = BTreeMap::from([
		("version", Value::from(configs.version().value)),
		("time-zone", Value::from(configs.time_zone().value.clone())),
	]);
	let classes: BTreeMap<_, _> = configs
		.classes()
		.iter()
		.map(|(name, class)| (name.as_str(), &class.description))
		.collect();
	values.insert("defined classes", json(&classes)?);
	let mut contexts: BTreeMap<_, _> = configs
		.contexts()
		.iter()
		.map(|(name, context)| (name.clone(), context.context.clone()))
		.collect();
	if !contexts.contains_key("default") {
		contexts.insert(
			"default".to_owned(),
			crate::policy::default_context(configs.version().value)
				.map_err(|error| failure(&error))?,
		);
	}
	values.insert("contexts", json(&contexts)?);
	match (document, placement) {
		(DocumentId::Local(path), _) => {
			directory_values(configs, path, &mut values)?;
		}
		(DocumentId::Remote(_), Placement::Remote { owner, .. }) => {
			let remote = configs.remote_settings(owner);
			let rules: BTreeMap<_, _> = remote
				.rules
				.iter()
				.map(|(name, rules)| {
					(
						name,
						rules.iter().map(|rule| &rule.value).collect::<Vec<_>>(),
					)
				})
				.collect();
			values.insert("rules", json(&rules)?);
		}
		(DocumentId::Remote(_), Placement::OnFile | Placement::Lockfile { .. }) => {
		}
	}
	Ok(values)
}

/// Adds all effective directory values and the selected adapter and eligibility.
fn directory_values(
	configs: &ConfigSet,
	path: &RepoPath,
	values: &mut BTreeMap<&'static str, Value>,
) -> Result<(), MutateError> {
	let settings = configs.settings(&path.parent().unwrap_or(RepoPath::ROOT));
	let include: Vec<_> = settings
		.include
		.value
		.iter()
		.map(|pattern| {
			(
				pattern.as_str(),
				anchor(configs, pattern, &settings.include.origin),
			)
		})
		.collect();
	values.insert("documents.include", json(&include)?);
	let ignore: Vec<_> = settings
		.ignore
		.iter()
		.map(|pattern| {
			(
				pattern.value.as_str(),
				anchor(configs, &pattern.value, &pattern.origin),
			)
		})
		.collect();
	values.insert("documents.ignore", json(&ignore)?);
	let adapters: Vec<_> = settings
		.adapters
		.iter()
		.map(|mapping| {
			(
				mapping.value.pattern.as_str(),
				anchor(configs, &mapping.value.pattern, &mapping.origin),
				mapping.value.adapter.name(),
			)
		})
		.collect();
	values.insert("documents.adapters", json(&adapters)?);
	values.insert("adapter", json(&configs.adapter(path))?);
	values.insert(
		"document eligibility",
		Value::Bool(settings.eligibility(path).is_eligible()),
	);
	let rules: BTreeMap<_, _> = settings
		.rules
		.iter()
		.map(|(name, rules)| {
			(
				name,
				rules.iter().map(|rule| &rule.value).collect::<Vec<_>>(),
			)
		})
		.collect();
	values.insert("rules", json(&rules)?);
	let series: Vec<_> = settings
		.series
		.iter()
		.map(|series| {
			(
				&series.name,
				series.pattern.as_str(),
				series
					.anchor
					.strip_prefix(configs.project())
					.unwrap_or(series.anchor.as_str()),
				series.version_kind,
				series.lifecycle,
				&series.schedule,
			)
		})
		.collect();
	values.insert("series", json(&series)?);
	Ok(())
}
