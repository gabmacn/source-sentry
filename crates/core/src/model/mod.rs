//! The data Source Sentry reasons about: paths, object IDs, identities,
//! declarations, facts and outcomes.
//!
//! These types own their data and carry the serde
//! representations of the file formats.
//! Parsing whole files and validating where each field may appear belongs to
//! [`crate::adapters`] and [`crate::config`].

mod declaration;
mod identity;
mod oid;
mod path;
mod rebase;
mod rules;
mod series;

pub use declaration::{
	Cadence, CadenceSetting, Declaration, Lockfile, Member, PathSource,
	PatternSource, Rename, Retired, Review, Schedule, Seal, SourceDecl,
};
pub use identity::{DocumentId, IdentityError, RemoteIdentity, Service};
pub use oid::{CommitId, HashAlgorithm, ObjectId, OidError};
pub use path::{
	PathError, RepoPath, resolve_project_source, resolve_rooted, resolve_source,
};
pub use rebase::{RebaseError, rebase_path, rebase_pattern};
pub use rules::{ClassName, ClassNameError, Context, Fact, Outcome, Rule};
pub use series::{Lifecycle, VersionKind};

#[cfg(test)]
mod tests;
