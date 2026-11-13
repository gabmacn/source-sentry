//! The git backend of Source Sentry.
//!
//! It implements the core's [`Snapshot`] and [`History`] traits for a real
//! repository, and applies the core's write plans:
//!
//! - [`GitRepository::open`] finds a repository.
//! - [`GitRepository::working_tree_snapshot`] and
//!   [`GitRepository::commit_snapshot`] produce the two kinds of snapshot: the
//!   working tree on top of `HEAD`, and a committed tree for `--rev`.
//! - [`GitRepository`] itself implements [`History`].
//! - [`GitRepository::apply`] applies a [`WritePlan`] all or nothing. It is the
//!   only code in Source Sentry that writes files, and `check` never calls it.
//!   Files it creates are marked intent-to-add.
//! - [`GitRepository::default_reviewer`] reads the name to record from git's
//!   configuration, and [`GitRepository::remember_reviewer`] stores one there.
//! - [`GitRepository::path_commits`] lists the commits that changed a path.
//!
//! `git2` reads history and objects. The `git` executable, which must be on
//! the `PATH`, lists the index and computes canonical blob oids for the
//! working tree with `git hash-object --stdin-paths`, without `-w`, so that
//! git's own conversions apply. Git LFS is handled natively: the LFS
//! clean filter never runs, because it writes into `.git/lfs/objects`.
//! Where `git hash-object` and `git add` would disagree about line endings,
//! or a path is a submodule, a snapshot reports a canonicalization
//! diagnostic instead of an oid.
//!
//! SHA-1 and SHA-256 repositories are both supported; object IDs
//! carry the repository's algorithm.
//!
//! Nothing here writes git objects or `.git/lfs`. The index changes in one
//! place only: [`GitRepository::apply`] marks the files a plan creates
//! intent-to-add.
//!
//! [`Snapshot`]: source_sentry_core::ports::Snapshot
//! [`History`]: source_sentry_core::ports::History
//! [`WritePlan`]: source_sentry_core::ports::WritePlan

mod attributes;
mod canonical;
mod command;
mod eol;
mod error;
mod history;
mod ids;
mod index;
mod intent;
mod lfs;
mod log;
mod presence;
mod repository;
mod snapshot;
mod write;

pub use log::{PathCommit, PathCommits};
pub use repository::GitRepository;
pub use snapshot::{CommitSnapshot, WorkingTreeSnapshot};
pub use write::{ApplyError, Unwritable};
