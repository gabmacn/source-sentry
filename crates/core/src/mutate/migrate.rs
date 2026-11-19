//! Schema migration of lockfiles.
//!
//! Lockfile schema 1 is the only schema so far, so nothing migrates yet. The
//! framework is in place: `migrate` reads every lockfile's `schema` before
//! interpreting anything else, refuses a schema it does not know, validates
//! a current lockfile as the adapters read it, reports every schema, and
//! plans nothing when every lockfile is current. When schema 2 exists, a
//! lockfile at schema 1 is rewritten in [`migrate`], and its result is
//! validated by the adapters like every other plan.

use serde_json::Value;

use super::{MutateError, Planned};
use crate::adapters::lockfile::{LocalLockfile, RemoteLockfile, SCHEMA};
use crate::claims::{Inventory, LockfileKind};
use crate::config::project_of;
use crate::model::RepoPath;
use crate::ports::{Snapshot, WritePlan};

/// Every lockfile's schema.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchemaReport {
	/// The schema this version of Source Sentry reads and writes.
	pub current: u32,
	/// Every lockfile, local and remote, sorted by path.
	pub lockfiles: Vec<LockfileSchema>,
}

/// One lockfile's schema.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockfileSchema {
	/// The lockfile.
	pub path: RepoPath,
	/// Its `schema`.
	pub schema: u64,
}

/// Reports the schema of every lockfile in `inventory` and plans their
/// migration to the current schema. A lockfile that failed to parse is
/// refused rather than reported current. Every lockfile is current today, so
/// the plan is empty.
///
/// # Errors
///
/// Refuses a lockfile that is not JSON or has no whole-number `schema`
/// ([`MutateError::UnreadableSchema`]), a schema that this version cannot
/// migrate ([`MutateError::UnknownSchema`]), and a current lockfile that
/// the adapters refuse ([`MutateError::InvalidLockfile`]). Fails when the
/// backend cannot read a lockfile.
pub fn migrate<S: Snapshot>(
	snapshot: &S,
	inventory: &Inventory,
) -> Result<Planned<SchemaReport>, MutateError> {
	let mut lockfiles = Vec::with_capacity(inventory.lockfiles.len());
	for lockfile in &inventory.lockfiles {
		let bytes = snapshot.read(&lockfile.path)?;
		let schema = schema_of(&lockfile.path, &bytes)?;
		// Schema 1 is the first, so there is no older schema to migrate from,
		// and any other value is unknown. A migration to a later schema adds
		// a match arm for the schema it rewrites.
		if schema != u64::from(SCHEMA) {
			return Err(MutateError::UnknownSchema {
				path: lockfile.path.clone(),
				found: schema,
				current: SCHEMA,
			});
		}
		let project = project_of(&inventory.projects, &lockfile.path);
		let parsed = match &lockfile.kind {
			LockfileKind::Local { directory } => {
				LocalLockfile::parse_in_project(&project, directory, &bytes).map(drop)
			}
			LockfileKind::Remote { .. } => {
				RemoteLockfile::parse_in_project(&project, &bytes).map(drop)
			}
		};
		parsed.map_err(|error| MutateError::InvalidLockfile {
			path: lockfile.path.clone(),
			error,
		})?;
		lockfiles.push(LockfileSchema {
			path: lockfile.path.clone(),
			schema,
		});
	}
	Ok(Planned {
		plan: WritePlan::default(),
		summary: SchemaReport {
			current: SCHEMA,
			lockfiles,
		},
	})
}

/// The `schema` of a lockfile, read without interpreting anything else,
/// since another schema may lay its other fields out differently.
fn schema_of(path: &RepoPath, bytes: &[u8]) -> Result<u64, MutateError> {
	let unreadable = |reason: String| MutateError::UnreadableSchema {
		path: path.clone(),
		reason,
	};
	let value: Value = serde_json::from_slice(bytes)
		.map_err(|error| unreadable(format!("it is not valid JSON ({error})")))?;
	value
		.get("schema")
		.and_then(Value::as_u64)
		.ok_or_else(|| unreadable("it has no whole-number `schema`".to_owned()))
}
