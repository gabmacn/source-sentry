//! Configs and presets: strict YAML parsing and resolution with origins.
//!
//! # Files
//!
//! [`ConfigSet::load`] reads every config of a snapshot:
//!
//! - the repository config, `.source-sentry/source-sentry.yaml`;
//! - every preset, `.source-sentry/presets/<name>.yaml`, where `<name>` may
//!   contain subfolders;
//! - every paired config, `.source-sentry/remotes/<owner>.yaml`;
//! - every directory config, `<directory>/source-sentry.yaml` outside
//!   `.source-sentry/`, whatever the ignore patterns say.
//!
//! It also notes which directories hold a lockfile, `source-sentry.lock`,
//! because such a directory is a root by default.
//!
//! Each file is parsed strictly: unsupported tags, YAML 1.1 booleans, duplicate
//! keys, merge keys and second documents are rejected, and every kind of file
//! allows only its own keys. An error names the file, the setting, the import
//! chain where one applies, and a correction.
//!
//! # Resolution
//!
//! [`ConfigSet::settings`] computes a directory's effective settings, each with
//! its [`Origin`]. Each setting resolves in this order, later overriding
//! earlier:
//!
//! 1. the built-in defaults;
//! 2. the repository config, after its own imports;
//! 3. the configs of ancestor directories, each after its own imports, unless
//!    a directory on the way sets `inherit: false`;
//! 4. the directory's own imports, expanded depth-first in the order listed,
//!    with repeated imports preserved;
//! 5. the directory's own settings.
//!
//! Settings inherit one by one, so an empty config resets nothing. Ignore
//! patterns and adapter mappings accumulate instead, and the last matching
//! one decides. Rules accumulate too, each scoped to the subtree of the config
//! that adds it. Series belong to the scope of the config that defines or
//! imports them.
//!
//! [`ConfigSet::remote_settings`] resolves a remote owner's defaults: the
//! repository config, then the paired config's imports, then its own settings.

mod diagnose;
mod error;
mod load;
mod parse;
mod pattern;
mod projects;
mod resolve;

use crate::adapters::Adapter;

pub use error::{ConfigError, ConfigErrorKind, LoadError, Position};
pub use load::ConfigSet;
pub use parse::{AdapterMapping, ConfigKind};
pub use pattern::{PathPattern, SeriesPattern};
pub use projects::{project_of, project_roots};
pub use resolve::{
	ClassDefinition, ContextDefinition, DirectorySettings, DocumentDefaults,
	Eligibility, Origin, RemoteSettings, Resolved, RootStatus, Scope, Series,
};

/// The schema and default-policy version of a repository that has no repository
/// config.
pub const IMPLICIT_VERSION: u32 = 1;

/// Every value of `version` that this build supports.
pub const SUPPORTED_VERSIONS: &[u32] = &[1];

/// The time zone that cadence dates use when the repository config sets none.
pub const DEFAULT_TIME_ZONE: &str = "UTC";

/// The context that is always declared, because Source Sentry builds it in. Its
/// rules belong to the policy module; a repository config that defines a
/// context of this name replaces it.
pub const DEFAULT_CONTEXT: &str = "default";

/// The document types and adapters that the built-in defaults set, as
/// `(pattern, adapter)` pairs. The patterns are also the built-in `include`
/// list.
pub const BUILT_IN_DOCUMENT_TYPES: &[(&str, Adapter)] = &[
	("*.md", Adapter::MdUtf8),
	("*.txt", Adapter::MdUtf8),
	("*.pdf", Adapter::Opaque),
];

#[cfg(test)]
mod tests;
