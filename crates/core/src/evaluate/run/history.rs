//! When new obligations and completions were introduced: predicates that read
//! historical declarations.
//!
//! Each [`Query`] names a document, the files that may declare it, and a
//! [`Condition`]. [`starts`] turns every query into a predicate for
//! [`introduced_at`], so all of them share one first-parent walk. A
//! predicate reads the declaring files' blobs at each commit through
//! [`History::blob`] and parses them, and the parses are cached by blob oid,
//! so a lockfile that many documents share, or a file that does not change
//! between commits, is parsed once.
//!
//! # Declaring files
//!
//! A local document's declaration is looked for in its own frontmatter, when
//! its adapter can carry one, and in the local lockfile whose authority
//! covers it. Both are read whatever the document's current mode, so a
//! declaration moved between frontmatter and a lockfile in one commit keeps
//! its history. A remote document's declaration is read
//! from its owner lockfile.
//!
//! Former sites are read too, so that a move between lockfiles also keeps
//! its history: for a local document, the lockfile of every directory above
//! it; for a remote document, every other owner's lockfile.
//!
//! # Files that cannot be read
//!
//! A declaring file that fails to parse at a commit never helps the document:
//! an obligation is taken to exist there, so its clock starts no later, and a
//! seal or a completion is taken to be absent, so it counts no earlier.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use chrono::{DateTime, Utc};

use crate::adapters::frontmatter;
use crate::adapters::lockfile::{LocalLockfile, RemoteLockfile};
use crate::evaluate::clock::{
	ClockError, ObligationStart, RunPredicate, introduced_at,
};
use crate::model::{
	Declaration, DocumentId, Retired, Review, Seal, SourceDecl,
	resolve_project_source,
};
use crate::ports::{
	BackendError, BackendErrorKind, Commit, History, ObjectId, RepoPath, Revision,
};
use crate::{DIR_NAME, REMOTES_DIR_NAME};

mod provenance;

use provenance::{Evidence, Provenance};

/// A file that may hold a document's declaration.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct Site {
	/// The project that owns the declaring file.
	pub(super) project: RepoPath,
	/// The file.
	pub(super) path: RepoPath,
	/// How to read it.
	pub(super) kind: SiteKind,
	/// Whether this is only a lockfile that may have held the declaration
	/// before a move: one above a local document that is not the current
	/// one, or another owner's lockfile for a remote document. Such a file
	/// counts only where it parses and has the document; when it cannot be
	/// read, it is ignored, because it may never have concerned the
	/// document.
	pub(super) former: bool,
}

/// How a declaring file is read.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum SiteKind {
	/// The document's own frontmatter.
	Frontmatter,
	/// A local lockfile in this directory.
	LocalLockfile {
		/// The lockfile's directory, which its keys are relative to.
		directory: RepoPath,
	},
	/// A remote owner lockfile.
	RemoteLockfile,
}

/// What a predicate asks of a document's declaration at each commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Condition {
	/// A declaration of the document exists: "the addition of an
	/// unattested declaration".
	Declared,
	/// The declaration has a path source that resolves to this path.
	PathSource(RepoPath),
	/// The declaration has this pattern, as declared, and `member` exists:
	/// "the commit where a file began to match".
	Pattern {
		/// The pattern as declared.
		pattern: String,
		/// The matching file.
		member: RepoPath,
	},
	/// A seal is recorded for the document, in its lockfile entry or its
	/// tombstones, matching this seal exactly.
	Sealed {
		/// The marker to match, including its oid and author.
		seal: Seal,
		/// The number of identical markers that must have been recorded.
		occurrence: usize,
	},
	/// This completion is recorded in the declaration or a tombstone.
	Completed {
		/// The marker to match, including its author.
		completed: Review,
		/// The number of identical markers that must have been recorded.
		occurrence: usize,
	},
}

impl Condition {
	/// Whether the condition marks a new obligation, whose clock a file that
	/// cannot be read must never delay, rather than a completion, which such
	/// a file must never advance.
	fn is_obligation(&self) -> bool {
		match self {
			Self::Declared | Self::PathSource(_) | Self::Pattern { .. } => true,
			Self::Sealed { .. } | Self::Completed { .. } => false,
		}
	}

	/// How many matching markers must exist across all declaring sites.
	fn required_occurrences(&self) -> usize {
		match self {
			Self::Sealed { occurrence, .. } | Self::Completed { occurrence, .. } => {
				*occurrence
			}
			Self::Declared | Self::PathSource(_) | Self::Pattern { .. } => 1,
		}
	}
}

/// One question for [`starts`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Query {
	/// The declaration's current project and all known project boundaries.
	pub(super) project: RepoPath,
	/// Project roots used to resolve former declarations.
	pub(super) projects: std::collections::BTreeSet<RepoPath>,
	/// The current declaration's explicit rename chain, if still live.
	pub(super) renamed: Vec<crate::model::Rename>,
	/// The document whose declaration is read.
	pub(super) document: DocumentId,
	/// The files that may declare it.
	pub(super) sites: Vec<Site>,
	/// What must hold.
	pub(super) condition: Condition,
}

/// When each query's condition started to hold without interruption up to
/// the evaluated revision, with one walk for all of them.
///
/// The result has one entry per query, in the order given, as
/// [`introduced_at`] describes.
///
/// # Errors
///
/// Fails when the backend fails, including when a declaring file's blob is
/// missing from the object database, so a required object is missing.
pub(super) fn starts<H: History>(
	history: &H,
	revision: &Revision,
	at: DateTime<Utc>,
	queries: &[Query],
) -> Result<Vec<Result<ObligationStart, ClockError>>, BackendError> {
	let reader = Reader {
		history,
		cache: RefCell::new(HashMap::new()),
	};
	let queries = with_deleted_owners(history, revision, queries)?;
	let mut predicates: Vec<Predicate<'_, '_, H>> = queries
		.iter()
		.map(|query| Predicate::new(query, &reader))
		.collect::<Result<_, _>>()?;
	introduced_at(history, revision, at, &mut predicates)
}

/// `queries`, where each remote document also reads, as former sites, the
/// owner lockfiles that history holds but the evaluated snapshot no longer
/// does, such as the one a move to another owner emptied and removed.
fn with_deleted_owners<H: History>(
	history: &H,
	revision: &Revision,
	queries: &[Query],
) -> Result<Vec<Query>, BackendError> {
	let mut queries = queries.to_vec();
	let head = match revision {
		Revision::WorkingTree { head } => head.as_ref(),
		Revision::Commit(commit) => Some(commit),
	};
	let remote = |query: &Query| matches!(query.document, DocumentId::Remote(_));
	let Some(head) = head.filter(|_| queries.iter().any(remote)) else {
		return Ok(queries);
	};
	let projects: std::collections::BTreeSet<_> = queries
		.iter()
		.filter(|query| remote(query))
		.flat_map(|query| query.sites.iter().map(|site| &site.project))
		.collect();
	let mut lockfiles = std::collections::BTreeSet::new();
	for project in projects {
		let directory = project
			.join(&format!("{DIR_NAME}/{REMOTES_DIR_NAME}"))
			.map_err(|error| {
				BackendError::new(BackendErrorKind::Io, error.to_string())
			})?;
		lockfiles.extend(
			history
				.files_ever_in(head, &directory)?
				.into_iter()
				.filter(|path| {
					path
						.file_name()
						.is_some_and(|name| name.ends_with(LOCKFILE_EXTENSION))
				}),
		);
	}
	for query in queries.iter_mut().filter(|query| remote(query)) {
		for lockfile in &lockfiles {
			if !query.sites.iter().any(|site| &site.path == lockfile) {
				query.sites.push(Site {
					project: lockfile
						.parent()
						.and_then(|path| path.parent())
						.and_then(|path| path.parent())
						.unwrap_or(RepoPath::ROOT),
					path: lockfile.clone(),
					kind: SiteKind::RemoteLockfile,
					former: true,
				});
			}
		}
	}
	Ok(queries)
}

/// The extension of a remote owner lockfile, `<owner>.lock`.
const LOCKFILE_EXTENSION: &str = ".lock";

/// A declaring file, parsed.
enum Parsed {
	/// Frontmatter, with its declaration if it has one, boxed because a
	/// declaration is much larger than the other variants.
	Frontmatter(Option<Box<Declaration>>),
	/// A local lockfile.
	Local(LocalLockfile),
	/// A remote owner lockfile.
	Remote(RemoteLockfile),
	/// The file does not parse.
	Unreadable,
}

/// What a declaring file says about one document.
enum Entry<'p> {
	/// The file does not parse.
	Unreadable,
	/// The document's declaration and tombstones, which may be absent.
	Found {
		declaration: Option<&'p Declaration>,
		retired: &'p [Retired],
	},
}

impl Parsed {
	/// What the file says about `document`.
	fn entry(&self, document: &DocumentId) -> Entry<'_> {
		match (self, document) {
			(Self::Unreadable, _) => Entry::Unreadable,
			(Self::Frontmatter(declaration), _) => Entry::Found {
				declaration: declaration.as_deref(),
				retired: &[],
			},
			(Self::Local(lockfile), DocumentId::Local(path)) => Entry::Found {
				declaration: lockfile.documents.get(path),
				retired: lockfile.retired.get(path).map_or(&[], Vec::as_slice),
			},
			(Self::Remote(lockfile), DocumentId::Remote(identity)) => Entry::Found {
				declaration: lockfile.documents.get(identity),
				retired: lockfile.retired.get(identity).map_or(&[], Vec::as_slice),
			},
			(Self::Local(_), DocumentId::Remote(_))
			| (Self::Remote(_), DocumentId::Local(_)) => Entry::Found {
				declaration: None,
				retired: &[],
			},
		}
	}
}

/// Reads and parses declaring files' blobs, caching each parse by blob oid
/// and the way it was read.
struct Reader<'h, H> {
	history: &'h H,
	/// Frontmatter parses also depend on the document, whose path relative
	/// sources are validated against, so its path is part of the key.
	cache: RefCell<HashMap<ParseKey, Rc<Parsed>>>,
}

/// A parse depends on the blob, adapter, document directory and project root.
type ParseKey = (ObjectId, SiteKind, RepoPath, RepoPath);

impl<H: History> Reader<'_, H> {
	/// The blob `oid` of `site`, parsed.
	///
	/// # Errors
	///
	/// Fails when the backend fails, or when the object database lacks the
	/// blob.
	fn parse(
		&self,
		oid: &ObjectId,
		site: &Site,
	) -> Result<Rc<Parsed>, BackendError> {
		let key = (
			oid.clone(),
			site.kind.clone(),
			site.path.clone(),
			site.project.clone(),
		);
		if let Some(parsed) = self.cache.borrow().get(&key) {
			return Ok(Rc::clone(parsed));
		}
		let bytes = self.history.blob(oid)?.ok_or_else(|| {
			BackendError::new(
				BackendErrorKind::MissingObject,
				format!(
					"blob {oid} of {} is missing from the object database",
					site.path
				),
			)
		})?;
		let parsed = Rc::new(match &site.kind {
			SiteKind::Frontmatter => {
				frontmatter::read_in_project(&site.project, &site.path, &bytes)
					.map_or(Parsed::Unreadable, |declaration| {
						Parsed::Frontmatter(declaration.map(Box::new))
					})
			}
			SiteKind::LocalLockfile { directory } => {
				LocalLockfile::parse_in_project(&site.project, directory, &bytes)
					.map_or(Parsed::Unreadable, Parsed::Local)
			}
			SiteKind::RemoteLockfile => {
				RemoteLockfile::parse_in_project(&site.project, &bytes)
					.map_or(Parsed::Unreadable, Parsed::Remote)
			}
		});
		self.cache.borrow_mut().insert(key, Rc::clone(&parsed));
		Ok(parsed)
	}
}

/// A [`Query`] as a [`RunPredicate`].
struct Predicate<'q, 'r, H> {
	query: &'q Query,
	reader: &'r Reader<'r, H>,
	/// The sites' paths, in order, then the pattern member the condition
	/// itself reads, if any.
	paths: Vec<RepoPath>,
	/// The identity active at this point in the backwards walk.
	provenance: Provenance,
}

impl<'q, 'r, H> Predicate<'q, 'r, H> {
	fn new(
		query: &'q Query,
		reader: &'r Reader<'r, H>,
	) -> Result<Self, BackendError> {
		let mut paths: Vec<RepoPath> =
			query.sites.iter().map(|site| site.path.clone()).collect();
		match &query.condition {
			Condition::Pattern { member, .. } => paths.push(member.clone()),
			Condition::Declared
			| Condition::PathSource(_)
			| Condition::Sealed { .. }
			| Condition::Completed { .. } => {}
		}
		Ok(Self {
			query,
			reader,
			paths,
			provenance: Provenance::new(query)?,
		})
	}

	/// How many times `declaration` and `retired` match the condition, given
	/// whether the condition's own path exists.
	fn occurrences(
		&self,
		project: &RepoPath,
		document: &DocumentId,
		declaration: Option<&Declaration>,
		retired: &[Retired],
		own_path_exists: bool,
	) -> usize {
		match &self.query.condition {
			Condition::Declared => usize::from(declaration.is_some()),
			Condition::PathSource(path) => {
				usize::from(declaration.is_some_and(|declaration| {
					declaration.sources.iter().any(|source| match source {
						SourceDecl::Path(source) => {
							resolve_project_source(
								project,
								local_path(document),
								&source.path,
							)
							.ok()
							.as_ref() == Some(path)
						}
						SourceDecl::Pattern(_) => false,
					})
				}))
			}
			Condition::Pattern { pattern, .. } => usize::from(
				own_path_exists
					&& declaration.is_some_and(|declaration| {
						declaration.sources.iter().any(|source| match source {
							SourceDecl::Pattern(source) => {
								crate::claims::SourceGlob::compile_project(
									project,
									local_path(document),
									&source.pattern,
								)
								.is_ok_and(|glob| glob.rooted() == *pattern)
							}
							SourceDecl::Path(_) => false,
						})
					}),
			),
			Condition::Sealed { seal, .. } => {
				usize::from(
					declaration
						.is_some_and(|declaration| declaration.seal.as_ref() == Some(seal)),
				) + retired
					.iter()
					.filter(|retired| retired.seal.as_ref() == Some(seal))
					.count()
			}
			Condition::Completed { completed, .. } => {
				usize::from(declaration.is_some_and(|declaration| {
					declaration.completed.as_ref() == Some(completed)
				})) + retired
					.iter()
					.filter(|retired| retired.completed.as_ref() == Some(completed))
					.count()
			}
		}
	}
}

impl<H: History> RunPredicate for Predicate<'_, '_, H> {
	fn paths(&self) -> &[RepoPath] {
		&self.paths
	}

	fn holds(
		&mut self,
		commit: &Commit,
		blobs: &[Option<&ObjectId>],
	) -> Result<bool, BackendError> {
		let own_path_exists = blobs
			.get(self.query.sites.len())
			.is_some_and(Option::is_some);
		loop {
			let historical;
			let view = if self.provenance.original() {
				blobs[..self.query.sites.len()].to_vec()
			} else {
				historical = self
					.reader
					.history
					.blobs_at(&commit.id, &self.provenance.paths())?;
				historical.iter().map(Option::as_ref).collect()
			};
			let mut evidence = Evidence::default();
			let mut nearest = false;
			for (site, oid) in self.provenance.sites().iter().zip(view) {
				let Some(oid) = oid else {
					continue;
				};
				let primary = !self.provenance.original()
					&& matches!(site.kind, SiteKind::LocalLockfile { .. })
					&& !std::mem::replace(&mut nearest, true);
				let parsed = self.reader.parse(oid, site)?;
				match parsed.entry(self.provenance.document()) {
					Entry::Unreadable => evidence.unreadable |= !site.former || primary,
					Entry::Found {
						declaration,
						retired,
					} => {
						evidence.occurrences += self.occurrences(
							&site.project,
							self.provenance.document(),
							declaration,
							retired,
							own_path_exists,
						);
						if let Some(declaration) = declaration {
							evidence.record(&site.project, declaration)?;
						}
					}
				}
			}
			if self.provenance.follow(&evidence)? {
				continue;
			}
			let pattern_file =
				!matches!(self.query.condition, Condition::Pattern { .. })
					|| own_path_exists;
			return Ok(
				evidence.occurrences >= self.query.condition.required_occurrences()
					|| (evidence.unreadable
						&& self.query.condition.is_obligation()
						&& pattern_file),
			);
		}
	}
}

/// The local path used to resolve a historical source, if it is local.
fn local_path(document: &DocumentId) -> Option<&RepoPath> {
	match document {
		DocumentId::Local(path) => Some(path),
		DocumentId::Remote(_) => None,
	}
}
