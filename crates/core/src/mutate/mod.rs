//! Mutations as write plans: attest, diff, seal, retire, track, init and
//! migrate.
//!
//! Every command is a planner. It reads the repository through a
//! [`Snapshot`] (and, for [`diff`], a [`History`](crate::ports::History)),
//! together with the [`Inventory`](crate::claims::Inventory) that
//! [`discover`](crate::claims::discover) built from that same snapshot, and
//! returns a [`Planned`] value: a [`WritePlan`] and a description of what the
//! plan changes, for the CLI to print and the user to confirm. Nothing here
//! writes; the backend applies the plan all or not
//! at all.
//!
//! # What every plan guarantees
//!
//! - **Expected oids.** Each [`PlannedWrite`](crate::ports::PlannedWrite)
//!   carries the file's current canonical oid, from
//!   [`Snapshot::canonical_oids`], or `None` for a file that does not exist
//!   yet, and each [`PlannedDelete`](crate::ports::PlannedDelete) the
//!   current canonical oid of the file it deletes. A file whose
//!   [`Canonical`](crate::ports::Canonical) value is not an oid is refused.
//! - **Only what changes.** A file whose contents would not change is left
//!   out. A lockfile that the change leaves with no documents and no
//!   tombstones is deleted, and one that would be created empty is not
//!   created.
//! - **Valid files.** Lockfiles are written with the adapters' `to_bytes`,
//!   which sorts them and validates every entry, and frontmatter with
//!   [`frontmatter::write`](crate::adapters::frontmatter::write). Every
//!   written file is parsed back before the plan is returned, and a
//!   frontmatter declaration must read back unchanged.
//! - **No new errors.** The plan is applied to an in-memory overlay of the
//!   snapshot, the configs are loaded again and discovery runs again. A plan
//!   that would introduce a configuration or integrity error, such as a
//!   double claim, an entry outside its lockfile's authority or a dependency
//!   cycle, is refused with those diagnostics.
//! - **Determinism.** Plans never read the clock: the instant is an input,
//!   and reviews are truncated to the second.
//!
//! # Refusals
//!
//! Every refusal is a [`MutateError`] that names what to fix. A command
//! refuses to act on a document that discovery reports a configuration or
//! integrity error about, except for the errors the command itself
//! resolves, which each command lists.

mod attest;
mod diff;
mod edits;
mod error;
mod expectation;
mod init;
mod migrate;
mod relocate;
mod repo;
mod retire;
mod seal;
mod select;
mod succession;
mod track;
mod untrack;
mod verify;

pub use attest::{
	AttestRequest, AttestSummary, AttestedEntry, AttestedSource, attest,
};
pub use diff::{Baseline, Current, DocumentDiff, EntryDiff, SourceDiff, diff};
pub use error::{Mismatch, MutateError};
pub use expectation::{Expectation, ExpectationError};
pub use init::{
	Candidates, ClaimedDocument, InitPreview, InitRequest, InitSummary, init,
	init_preview,
};
pub use migrate::{LockfileSchema, SchemaReport, migrate};
pub use relocate::{
	Moved, Setting, SettingsPreview, TrackFromRequest, TrackFromSummary,
	track_from,
};
pub use repo::Placement;
pub use retire::{RetireRequest, RetireSummary, retire};
pub use seal::{SealRequest, SealSummary, seal};
pub use select::Selection;
pub use succession::Inherited;
pub use track::{
	CarriedTombstone, PatternMatches, Registered, SourceSpec, Target,
	TrackRequest, TrackSummary, track,
};
pub use untrack::{UntrackRequest, UntrackSummary, untrack};

use crate::ports::WritePlan;

#[cfg(doc)]
use crate::ports::Snapshot;

/// A command's write plan, with a description of what it changes.
///
/// The CLI prints the summary and asks the user to confirm before the
/// backend applies the plan. An empty plan changes nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Planned<T> {
	/// The files to write, each with the canonical oid it must still have.
	pub plan: WritePlan,
	/// What the plan changes, for people to read.
	pub summary: T,
}

#[cfg(test)]
mod tests;
