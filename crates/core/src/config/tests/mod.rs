//! Tests of config parsing, validation and resolution, against in-memory
//! snapshots.

mod own_defaults;
mod parsing;
mod paths;
mod projects;
mod resolution;
mod sharing;
mod validation;

use super::{ConfigError, ConfigErrorKind, ConfigSet, LoadError};
use crate::testing::FakeSnapshot;

/// The repository config's path.
const REPOSITORY: &str = ".source-sentry/source-sentry.yaml";

/// A repository config that declares two classes and one context.
const BASE: &str = "\
version: 1
define:
  classes:
    internal-docs: {}
    privacy:
      description: 'Personal data.'
  contexts:
    production:
      fallback: warn
";

/// A working tree holding `files`, given as `(path, contents)`.
fn snapshot(files: &[(&str, &str)]) -> FakeSnapshot {
	files.iter().fold(
		FakeSnapshot::working_tree(None),
		|snapshot, (file, contents)| snapshot.with_file(file, contents.as_bytes()),
	)
}

/// Loads `files`, which must be valid.
fn load(files: &[(&str, &str)]) -> ConfigSet {
	match ConfigSet::load(&snapshot(files)) {
		Ok(configs) => configs,
		Err(error) => panic!("expected valid configs, got:\n{error}"),
	}
}

/// Loads `files`, which must be invalid, and returns the errors.
fn errors(files: &[(&str, &str)]) -> Vec<ConfigError> {
	match ConfigSet::load(&snapshot(files)) {
		Err(LoadError::Invalid(errors)) => errors,
		Err(error) => panic!("expected configuration errors, got {error}"),
		Ok(_) => panic!("expected configuration errors, but the configs loaded"),
	}
}

/// Loads `files`, which must have exactly one error, and returns it.
fn only_error(files: &[(&str, &str)]) -> ConfigError {
	let mut errors = errors(files);
	assert_eq!(errors.len(), 1, "expected one error, got {errors:#?}");
	errors.remove(0)
}

/// Parses `contents` as the repository config, after [`BASE`]'s
/// declarations, and returns its only error.
fn repository_error(contents: &str) -> ConfigError {
	only_error(&[(REPOSITORY, &format!("{BASE}{contents}"))])
}

/// Parses `contents` as the directory config of `docs`, beside [`BASE`],
/// and returns its only error.
fn directory_error(contents: &str) -> ConfigError {
	only_error(&[(REPOSITORY, BASE), ("docs/source-sentry.yaml", contents)])
}

/// Asserts that `error` is a forbidden-setting error for `key`.
fn assert_forbidden(error: &ConfigError, key: &str) {
	assert!(
		matches!(&error.kind, ConfigErrorKind::ForbiddenSetting { key: found, .. } if found == key),
		"expected `{key}` to be forbidden, got {error:#?}"
	);
	assert_eq!(error.setting.as_deref(), Some(key));
	assert!(error.correction.is_some());
}
