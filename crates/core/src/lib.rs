//! Tracks documentation and records against the repository sources they
//! describe.
//!
//! The core performs no I/O and does not depend on `git2`. It reads a
//! repository through the traits in [`ports`], which the git backend
//! implements, and it expresses every change as a write plan that the backend
//! applies. The CLI is a separate workspace member.

/// The name of the product.
pub const NAME: &str = "Source Sentry";
/// The slug of the product, used in CLI commands and configuration files where
/// spaces are not allowed.
pub const SLUG: &str = "source-sentry";
/// Shorthand slug of the product, used in CLI commands and configuration files
/// where spaces are not allowed.
pub const SLUG_SHORT: &str = "ss";
/// The file name of a YAML config: either a directory config, or the
/// repository config inside [`DIR_NAME`].
pub const CONFIG_NAME: &str = "source-sentry.yaml";
/// The file name of a lockfile, which Source Sentry writes as JSON.
pub const LOCKFILE_NAME: &str = "source-sentry.lock";
/// The committed directory at the repository root that holds the repository
/// config, presets and remote lockfiles.
pub const DIR_NAME: &str = ".source-sentry";
/// The folder inside [`DIR_NAME`] that holds importable presets.
pub const PRESETS_DIR_NAME: &str = "presets";
/// The folder inside [`DIR_NAME`] that holds remote owner lockfiles and their
/// paired configs.
pub const REMOTES_DIR_NAME: &str = "remotes";
/// The folder inside [`DIR_NAME`] for the disposable local cache, which
/// ignores itself through its own `.gitignore`.
pub const CACHE_DIR_NAME: &str = "cache";

pub mod adapters;
pub mod claims;
pub mod config;
pub mod evaluate;
pub mod model;
pub mod mutate;
pub mod policy;
pub mod ports;
pub mod report;

mod yaml;

#[cfg(any(test, feature = "testing"))]
pub mod testing;
