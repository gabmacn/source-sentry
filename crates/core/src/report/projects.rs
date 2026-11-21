//! Project-relative paths at the JSON serialization boundary.

use std::collections::BTreeSet;

use serde::Serialize;
use serde_json::Value;

use crate::config::project_of;
use crate::model::{RemoteIdentity, RepoPath, rebase_path};

/// Serializes a report with project-relative paths and explicit project identities.
/// Source declarations retain their written spellings; external paths use `../`.
/// The repository project is written `.`. Command arrays already carry the
/// caller's intended working directory and are preserved.
///
/// # Errors
///
/// Fails when the report cannot be represented as JSON.
pub fn to_project_json<T: Serialize>(
	report: &T,
	project: &RepoPath,
	roots: &BTreeSet<RepoPath>,
) -> Result<String, serde_json::Error> {
	serde_json::to_string_pretty(&to_project_value(report, project, roots)?)
}

/// [`to_project_json`], as a value rather than text, for a report that
/// rebases its parts on different projects.
///
/// # Errors
///
/// Fails when the report cannot be represented as JSON.
pub(super) fn to_project_value<T: Serialize>(
	report: &T,
	project: &RepoPath,
	roots: &BTreeSet<RepoPath>,
) -> Result<Value, serde_json::Error> {
	let mut value = serde_json::to_value(report)?;
	rebase(&mut value, "", project, roots);
	Ok(value)
}

/// Re-expresses a canonical repository path relative to a project.
fn relative(value: &str, project: &RepoPath) -> String {
	rebase_path(value, &RepoPath::ROOT, project)
		.unwrap_or_else(|_| value.to_owned())
}

/// Re-expresses a `document` field, which holds either a local path or a
/// remote identity. A document subject says which through its `mode`. The
/// diff report has no mode beside its document, so there only the spelling
/// can tell an identity from a path.
fn document(value: &str, mode: Option<&str>, project: &RepoPath) -> String {
	let remote = match mode {
		Some(mode) => mode == "remote",
		None => value.parse::<RemoteIdentity>().is_ok(),
	};
	if remote {
		value.to_owned()
	} else {
		relative(value, project)
	}
}

/// A rebased pattern, written from the project root with a leading `/`, or,
/// when it leaves the project, with the parents that `relative` gives it,
/// as the paths beside it are.
fn rooted(pattern: &str) -> String {
	if pattern == ".." || pattern.starts_with("../") {
		pattern.to_owned()
	} else {
		format!("/{pattern}")
	}
}

/// Visits the report's path-bearing fields without touching declarations or text.
fn rebase(
	value: &mut Value,
	key: &str,
	project: &RepoPath,
	roots: &BTreeSet<RepoPath>,
) {
	match value {
		Value::Object(fields) => {
			let own = if fields.contains_key("project") && fields.contains_key("file")
			{
				fields
					.get("file")
					.and_then(Value::as_str)
					.and_then(|file| RepoPath::new(file).ok())
					.map(|file| project_of(roots, &file))
			} else {
				None
			};
			let project = own.as_ref().unwrap_or(project);
			let mode = fields
				.get("mode")
				.and_then(Value::as_str)
				.map(str::to_owned);
			for (key, value) in fields {
				if key == "project" {
					*value = Value::String(super::directory(project));
				} else if key == "document"
					&& let Value::String(text) = value
				{
					*text = document(text, mode.as_deref(), project);
				} else {
					rebase(value, key, project, roots);
				}
			}
		}
		Value::Array(values) if key != "attest" => {
			for value in values {
				rebase(value, key, project, roots);
			}
		}
		Value::String(text) => match key {
			"file" | "path" | "declared-in" | "root" | "anchor" | "inherit-from"
			| "stale-upstreams" | "due-upstreams" | "import-chain" => {
				*text = relative(text, project);
			}
			"scope" if text.starts_with("directory:") => {
				*text = format!("directory:{}", relative(&text[10..], project));
			}
			"pattern" if text.starts_with('/') => {
				*text = rooted(&relative(&text[1..], project));
			}
			"expectations" => {
				if let Some((path, state)) = text.rsplit_once('@') {
					*text = format!("{}@{state}", relative(path, project));
				}
			}
			_ => {}
		},
		_ => {}
	}
}
