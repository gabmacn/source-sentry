//! The inventory: what discovery found in one snapshot.

use std::collections::{BTreeMap, BTreeSet};

use super::{Diagnostic, Location, Version};
use crate::config::Series;
use crate::model::{
	Declaration, DocumentId, ObjectId, RepoPath, Retired, Review,
};
use crate::ports::Revision;

/// Everything discovery found in one snapshot: claims, tombstones,
/// dependencies between documents, unclaimed files, series membership and
/// diagnostics.
///
/// Built by [`discover`](super::discover). Every list is sorted, so two
/// discoveries of the same snapshot are equal. Problems never stop
/// discovery: each is recorded in [`Inventory::diagnostics`], and the rest of
/// the inventory holds whatever could still be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inventory {
	/// The snapshot this inventory describes.
	pub revision: Revision,
	/// Every lockfile, local and remote, sorted by path, including those that
	/// failed to parse.
	pub lockfiles: Vec<LockfileInfo>,
	/// Every claim, sorted by document, then by the file and field that
	/// declare it. A document with more than one claim has a
	/// [`DoubleClaim`](super::DiagnosticKind::DoubleClaim) diagnostic, and
	/// all of its claims are kept.
	pub claims: Vec<Claim>,
	/// Every tombstone, sorted by document, then by lockfile.
	pub tombstones: Vec<Tombstone>,
	/// Every edge from a document to a source that is itself a
	/// lockfile-tracked document, sorted and without repeats.
	/// Sealed records contribute none.
	pub dependencies: Vec<Dependency>,
	/// Every unclaimed file, sorted by path.
	pub unclaimed: Vec<Unclaimed>,
	/// Every series in effect anywhere, with its ordered members, sorted by
	/// scope and name. A series without members is
	/// listed too.
	pub series: Vec<SeriesMembers>,
	/// Every problem found, sorted by location, then by kind.
	pub diagnostics: Vec<Diagnostic>,
}

impl Inventory {
	/// Whether any diagnostic is a configuration or integrity error, which
	/// exits 3.
	#[must_use]
	pub fn has_errors(&self) -> bool {
		self
			.diagnostics
			.iter()
			.any(|diagnostic| diagnostic.severity().is_fatal())
	}

	/// The claims of `document`: none when it is untracked, one normally,
	/// and more for a double claim. The iterator keeps its own copy of
	/// `document`, which may therefore be a temporary.
	pub fn claims_of<'a>(
		&'a self,
		document: &DocumentId,
	) -> impl Iterator<Item = &'a Claim> + use<'a> {
		let document = document.clone();
		self
			.claims
			.iter()
			.filter(move |claim| claim.document == document)
	}

	/// The documents that `document` depends on through its sources, which
	/// [`ResolvedSource`]s name and which are themselves lockfile-tracked
	/// documents. Evaluation derives `upstream-stale` from them.
	/// The iterator keeps its own copy of `document`, which may therefore be
	/// a temporary.
	pub fn upstreams_of<'a>(
		&'a self,
		document: &DocumentId,
	) -> impl Iterator<Item = &'a RepoPath> + use<'a> {
		let document = document.clone();
		self
			.dependencies
			.iter()
			.filter(move |dependency| dependency.dependent == document)
			.map(|dependency| &dependency.upstream)
	}

	/// The local lockfile whose authority covers `path`: the nearest one in
	/// a directory that contains it. This is where `track` would
	/// list `path`. `None` when no local lockfile contains it.
	#[must_use]
	pub fn lockfile_for(&self, path: &RepoPath) -> Option<&LockfileInfo> {
		nearest_lockfile(&self.lockfiles, path)
	}

	/// The series that `path` belongs to, with its membership, when it is a
	/// member of exactly one.
	#[must_use]
	pub fn series_of(
		&self,
		path: &RepoPath,
	) -> Option<(&SeriesMembers, &SeriesMember)> {
		self.series.iter().find_map(|series| {
			series
				.members
				.iter()
				.find(|member| &member.path == path)
				.map(|member| (series, member))
		})
	}

	/// The newest strictly earlier member of `path`'s series with a live
	/// declaration. Sealed declarations qualify; tombstones do not.
	#[must_use]
	pub fn predecessor(&self, path: &RepoPath) -> Option<&Claim> {
		let (series, member) = self.series_of(path)?;
		series
			.members
			.iter()
			.rev()
			.filter(|earlier| earlier.version < member.version)
			.find_map(|earlier| {
				self
					.claims_of(&DocumentId::Local(earlier.path.clone()))
					.next()
			})
	}
}

/// The local lockfile among `lockfiles` whose directory is the nearest that
/// strictly contains `path`, parsed or not.
pub(super) fn nearest_lockfile<'a>(
	lockfiles: &'a [LockfileInfo],
	path: &RepoPath,
) -> Option<&'a LockfileInfo> {
	lockfiles
		.iter()
		.filter_map(|lockfile| match &lockfile.kind {
			LockfileKind::Local { directory }
				if path != directory && path.starts_with(directory) =>
			{
				Some((directory.components().count(), lockfile))
			}
			LockfileKind::Local { .. } | LockfileKind::Remote { .. } => None,
		})
		.max_by_key(|(depth, _)| *depth)
		.map(|(_, lockfile)| lockfile)
}

/// A lockfile that discovery found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockfileInfo {
	/// Where it lives.
	pub path: RepoPath,
	/// Whether it is local or a remote owner's.
	pub kind: LockfileKind,
	/// Whether it parsed. When it did not, none of its documents or
	/// tombstones appear in the inventory, an
	/// [`InvalidLockfile`](super::DiagnosticKind::InvalidLockfile)
	/// diagnostic says why, and a mutation must not rewrite it.
	pub parsed: bool,
}

/// The two kinds of lockfile.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LockfileKind {
	/// `<directory>/source-sentry.lock`, whose authority is its directory's
	/// subtree minus the subtrees of nested lockfiles.
	Local {
		/// The directory that holds it, which its keys are relative to.
		directory: RepoPath,
	},
	/// `.source-sentry/remotes/<owner>.lock`.
	Remote {
		/// The owner, which also names the paired config.
		owner: String,
	},
}

/// How a document is tracked.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Mode {
	/// By the `source-sentry` key of the document's own frontmatter.
	OnFile,
	/// By the `source-sentry.lock` whose authority covers the document.
	LocalLockfile,
	/// By a remote owner lockfile.
	RemoteLockfile {
		/// The owner, whose paired config supplies the document's defaults
		/// through [`remote_settings`](crate::config::ConfigSet::remote_settings).
		owner: String,
	},
}

/// Whether a claimed document exists in the snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Presence {
	/// The snapshot has the file.
	Present,
	/// A lockfile lists a local file that the snapshot lacks. The claim has a
	/// [`MissingDocument`](super::DiagnosticKind::MissingDocument) or
	/// [`MissingRecord`](super::DiagnosticKind::MissingRecord) diagnostic.
	Missing,
	/// A remote document, whose existence Source Sentry cannot know.
	Unknown,
}

/// One declaration of a document: on-file, or an entry of a lockfile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Claim {
	/// The document claimed.
	pub document: DocumentId,
	/// How it is tracked.
	pub mode: Mode,
	/// The file that holds the declaration: the document itself when
	/// on-file, otherwise the lockfile.
	pub declared_in: RepoPath,
	/// The declaration's field in that file: `source-sentry` in
	/// frontmatter, or `documents["<key>"]` in a lockfile.
	pub field: String,
	/// The declaration as written.
	pub declaration: Declaration,
	/// The declaration's sources, resolved, in declared order. A source that
	/// could not be resolved is left out and has a diagnostic; each entry
	/// names its index in [`Declaration::sources`].
	pub sources: Vec<ResolvedSource>,
	/// Whether the document exists.
	pub presence: Presence,
	/// Where each declaration field starts, by its diagnostic path, such as
	/// `source-sentry.classes[1]` or `documents["docs/a.md"].classes[1]`.
	pub lines: BTreeMap<String, u64>,
}

impl Claim {
	/// Where the declaration is.
	#[must_use]
	pub fn location(&self) -> Location {
		Location::field(self.declared_in.clone(), self.field.clone())
	}

	/// Where `field` of the declaration is, with its line when known.
	#[must_use]
	pub fn location_of(&self, field: String) -> Location {
		let mut location = self.location();
		location.line = self.lines.get(&field).copied();
		location.field = Some(field);
		location
	}

	/// Whether the document is a sealed record, whose sources are frozen.
	/// Only local lockfile entries can be.
	#[must_use]
	pub fn is_sealed(&self) -> bool {
		self.declaration.seal.is_some()
	}

	/// Every current source path, without repeats: each path source, and
	/// each current match of each pattern. A sealed record's
	/// patterns are not expanded, so only its path sources appear.
	#[must_use]
	pub fn source_paths(&self) -> BTreeSet<&RepoPath> {
		let mut paths = BTreeSet::new();
		for source in &self.sources {
			match source {
				ResolvedSource::Path(source) => {
					paths.insert(&source.path);
				}
				ResolvedSource::Pattern(source) => {
					paths.extend(source.matches.iter().flatten());
				}
			}
		}
		paths
	}
}

/// A declared source, resolved against the snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolvedSource {
	/// A single path.
	Path(ResolvedPath),
	/// A pattern, with its current matches and its attested members.
	Pattern(ResolvedPattern),
}

impl ResolvedSource {
	/// The source's index in [`Declaration::sources`].
	#[must_use]
	pub fn index(&self) -> usize {
		match self {
			Self::Path(source) => source.index,
			Self::Pattern(source) => source.index,
		}
	}
}

/// What a source was attested at, and by whom.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attestation {
	/// The attested canonical blob oid.
	pub oid: ObjectId,
	/// Who reviewed it, and when.
	pub reviewed: Review,
}

/// A path source, resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedPath {
	/// The index in [`Declaration::sources`].
	pub index: usize,
	/// The canonical path.
	pub path: RepoPath,
	/// Whether the snapshot has it. A missing source of a live obligation
	/// has a [`MissingSource`](super::DiagnosticKind::MissingSource)
	/// diagnostic; a sealed record's sources are frozen, so theirs is only
	/// recorded.
	pub exists: bool,
	/// The attestation, or `None` when the source was never attested.
	pub attestation: Option<Attestation>,
}

/// A pattern source, resolved and expanded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedPattern {
	/// The index in [`Declaration::sources`].
	pub index: usize,
	/// The pattern resolved against the repository root, written with a
	/// leading `/`, such as `/docs/*.md` for `*.md` declared in
	/// `docs/a.md`.
	pub glob: String,
	/// The tracked paths it matches now, or `None` for a sealed record,
	/// whose patterns are not expanded. Evaluation compares them
	/// with [`ResolvedPattern::members`] to find additions and removals.
	pub matches: Option<BTreeSet<RepoPath>>,
	/// The members as last attested, by canonical path. A member whose path
	/// cannot be resolved is left out and has a diagnostic.
	pub members: BTreeMap<RepoPath, Attestation>,
}

/// A tombstone left by `retire`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tombstone {
	/// The retired document.
	pub document: DocumentId,
	/// The lockfile that holds the tombstone.
	pub lockfile: RepoPath,
	/// Its field there: `retired["<key>"][<index>]`.
	pub field: String,
	/// Who retired it, and when, and the seal of a retired record.
	pub retired: Retired,
	/// Whether a file exists at the retired path again. A local path may be
	/// reused, and is then claimed anew or unclaimed. Remote
	/// documents are [`Presence::Unknown`].
	pub presence: Presence,
	/// Where each tombstone field starts, by its diagnostic path, such as
	/// `retired["docs/a.md"][0].completed`.
	pub lines: BTreeMap<String, u64>,
}

impl Tombstone {
	/// Where `field` of the tombstone is, with its line when known.
	#[must_use]
	pub fn location_of(&self, field: String) -> Location {
		let line = self.lines.get(&field).copied();
		Location::field(self.lockfile.clone(), field).at(line, None)
	}
}

/// One document depending on another through a source.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Dependency {
	/// The document with the source.
	pub dependent: DocumentId,
	/// The source, which is a lockfile-tracked document.
	pub upstream: RepoPath,
}

/// An eligible file inside a root that nothing claims.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Unclaimed {
	/// The file.
	pub path: RepoPath,
	/// The root whose scan found it, which supplies its default classes.
	pub root: RepoPath,
	/// The predecessor whose declaration registration would inherit, if any.
	pub inherit_from: Option<RepoPath>,
}

/// A series and its members, ordered by version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeriesMembers {
	/// The series, whose identity is its scope plus its name.
	pub series: Series,
	/// The members, from the oldest version to the newest, ties broken by
	/// path. A member whose version does not parse, or which matches more
	/// than one series, is left out and has a diagnostic; members with equal
	/// versions are kept and have a diagnostic.
	pub members: Vec<SeriesMember>,
}

/// One member of a series.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeriesMember {
	/// The member's path.
	pub path: RepoPath,
	/// Its version, parsed.
	pub version: Version,
	/// Whether it exists, is listed but missing, or is only retired.
	pub state: MemberState,
	/// Whether any declaration claims it. An unclaimed member is still a
	/// member.
	pub claimed: bool,
}

/// Why a path counts as a series member.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MemberState {
	/// The snapshot has the file.
	Present,
	/// A local lockfile lists the file, but the snapshot lacks it.
	Missing,
	/// Only a tombstone names the path, and the snapshot lacks it.
	Retired,
}
