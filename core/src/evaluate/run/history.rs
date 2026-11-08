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
	Declaration, DocumentId, Retired, SourceDecl, resolve_rooted, resolve_source,
};
use crate::ports::{
	BackendError, BackendErrorKind, Commit, History, ObjectId, RepoPath, Revision,
};

/// A file that may hold a document's declaration.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct Site {
	/// The file.
	pub(super) path: RepoPath,
	/// How to read it.
	pub(super) kind: SiteKind,
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
	/// tombstone.
	Sealed,
	/// The declaration has `completed`.
	Completed,
}

impl Condition {
	/// Whether the condition marks a new obligation, whose clock a file that
	/// cannot be read must never delay, rather than a completion, which such
	/// a file must never advance.
	fn is_obligation(&self) -> bool {
		match self {
			Self::Declared | Self::PathSource(_) | Self::Pattern { .. } => true,
			Self::Sealed | Self::Completed => false,
		}
	}
}

/// One question for [`starts`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Query {
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
	let mut predicates: Vec<Predicate<'_, '_, H>> = queries
		.iter()
		.map(|query| Predicate::new(query, &reader))
		.collect();
	introduced_at(history, revision, at, &mut predicates)
}

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
	/// The document's declaration and tombstone, either of which may be
	/// absent.
	Found {
		declaration: Option<&'p Declaration>,
		retired: Option<&'p Retired>,
	},
}

impl Parsed {
	/// What the file says about `document`.
	fn entry(&self, document: &DocumentId) -> Entry<'_> {
		match (self, document) {
			(Self::Unreadable, _) => Entry::Unreadable,
			(Self::Frontmatter(declaration), _) => Entry::Found {
				declaration: declaration.as_deref(),
				retired: None,
			},
			(Self::Local(lockfile), DocumentId::Local(path)) => Entry::Found {
				declaration: lockfile.documents.get(path),
				retired: lockfile.retired.get(path),
			},
			(Self::Remote(lockfile), DocumentId::Remote(identity)) => Entry::Found {
				declaration: lockfile.documents.get(identity),
				retired: lockfile.retired.get(identity),
			},
			(Self::Local(_), DocumentId::Remote(_))
			| (Self::Remote(_), DocumentId::Local(_)) => Entry::Found {
				declaration: None,
				retired: None,
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
	cache: RefCell<HashMap<(ObjectId, SiteKind, RepoPath), Rc<Parsed>>>,
}

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
		let key = (oid.clone(), site.kind.clone(), site.path.clone());
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
			SiteKind::Frontmatter => frontmatter::read(&site.path, &bytes)
				.map_or(Parsed::Unreadable, |declaration| {
					Parsed::Frontmatter(declaration.map(Box::new))
				}),
			SiteKind::LocalLockfile { directory } => {
				LocalLockfile::parse(directory, &bytes)
					.map_or(Parsed::Unreadable, Parsed::Local)
			}
			SiteKind::RemoteLockfile => {
				RemoteLockfile::parse(&bytes).map_or(Parsed::Unreadable, Parsed::Remote)
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
}

impl<'q, 'r, H> Predicate<'q, 'r, H> {
	fn new(query: &'q Query, reader: &'r Reader<'r, H>) -> Self {
		let mut paths: Vec<RepoPath> =
			query.sites.iter().map(|site| site.path.clone()).collect();
		match &query.condition {
			Condition::Pattern { member, .. } => paths.push(member.clone()),
			Condition::Declared
			| Condition::PathSource(_)
			| Condition::Sealed
			| Condition::Completed => {}
		}
		Self {
			query,
			reader,
			paths,
		}
	}

	/// Whether `declaration` and `retired` satisfy the condition, given
	/// whether the condition's own path exists.
	fn satisfied(
		&self,
		declaration: Option<&Declaration>,
		retired: Option<&Retired>,
		own_path_exists: bool,
	) -> bool {
		match &self.query.condition {
			Condition::Declared => declaration.is_some(),
			Condition::PathSource(path) => declaration.is_some_and(|declaration| {
				declaration.sources.iter().any(|source| match source {
					SourceDecl::Path(source) => {
						self.resolve(&source.path).as_ref() == Some(path)
					}
					SourceDecl::Pattern(_) => false,
				})
			}),
			Condition::Pattern { pattern, .. } => {
				own_path_exists
					&& declaration.is_some_and(|declaration| {
						declaration.sources.iter().any(|source| match source {
							SourceDecl::Pattern(source) => &source.pattern == pattern,
							SourceDecl::Path(_) => false,
						})
					})
			}
			Condition::Sealed => {
				declaration.is_some_and(|declaration| declaration.seal.is_some())
					|| retired.is_some_and(|retired| retired.seal.is_some())
			}
			Condition::Completed => {
				declaration.is_some_and(|declaration| declaration.completed.is_some())
			}
		}
	}

	/// A declared source path, resolved as discovery resolves it, or
	/// `None` when it does not resolve.
	fn resolve(&self, declared: &str) -> Option<RepoPath> {
		match &self.query.document {
			DocumentId::Local(document) => resolve_source(document, declared).ok(),
			DocumentId::Remote(_) => resolve_rooted(declared).ok(),
		}
	}
}

impl<H: History> RunPredicate for Predicate<'_, '_, H> {
	fn paths(&self) -> &[RepoPath] {
		&self.paths
	}

	fn holds(
		&mut self,
		_commit: &Commit,
		blobs: &[Option<&ObjectId>],
	) -> Result<bool, BackendError> {
		let sites = self.query.sites.len();
		let own_path_exists = blobs.get(sites).is_some_and(Option::is_some);
		let mut unreadable = false;
		for (site, blob) in self.query.sites.iter().zip(blobs) {
			let Some(oid) = blob else {
				continue;
			};
			let parsed = self.reader.parse(oid, site)?;
			match parsed.entry(&self.query.document) {
				Entry::Unreadable => unreadable = true,
				Entry::Found {
					declaration,
					retired,
				} => {
					if self.satisfied(declaration, retired, own_path_exists) {
						return Ok(true);
					}
				}
			}
		}
		// An unreadable file never delays an obligation's clock. A pattern's
		// obligation still needs its file to exist.
		let pattern_file = match self.query.condition {
			Condition::Pattern { .. } => own_path_exists,
			_ => true,
		};
		Ok(unreadable && self.query.condition.is_obligation() && pattern_file)
	}
}
