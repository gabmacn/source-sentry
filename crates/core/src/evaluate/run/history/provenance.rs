//! Explicit path boundaries on a backwards declaration-history walk.

use chrono::{DateTime, Utc};

use super::{Query, Site, SiteKind};
use crate::model::{
	Declaration, DocumentId, Rename, RepoPath, resolve_project_source,
};
use crate::ports::{BackendError, BackendErrorKind};

/// Evidence read at the active identity for one historical commit.
#[derive(Default)]
pub(super) struct Evidence {
	/// Matching obligations or completion markers across declaring sites.
	pub(super) occurrences: usize,
	/// Matching markers in live declarations, independently of tombstones.
	pub(super) live_occurrences: usize,
	/// An authoritative declaration could not be read.
	pub(super) unreadable: bool,
	/// Rename markers from readable declarations, with resolved identities.
	markers: Vec<Link>,
}

impl Evidence {
	/// Keeps only a declaration's explicit path boundaries, without copying
	/// its sources and reviews for every clock question.
	pub(super) fn record(
		&mut self,
		project: &RepoPath,
		declaration: &Declaration,
	) -> Result<(), BackendError> {
		for rename in &declaration.renamed {
			self.markers.push(Link::new(project, rename)?);
		}
		Ok(())
	}
}

/// A rename marker with its former path resolved, independent of spelling.
#[derive(PartialEq, Eq)]
struct Link {
	/// The former repository identity.
	from: RepoPath,
	/// Informational instant, used only to identify the exact marker.
	at: DateTime<Utc>,
	/// Informational recorder, used only to identify the exact marker.
	by: Option<String>,
}

impl Link {
	/// Resolves a validated marker at its declaring project.
	fn new(project: &RepoPath, rename: &Rename) -> Result<Self, BackendError> {
		Ok(Self {
			from: resolve_project_source(project, None, &rename.from).map_err(
				|error| BackendError::new(BackendErrorKind::Io, error.to_string()),
			)?,
			at: rename.at,
			by: rename.by.clone(),
		})
	}
}

/// The active document and the boundaries still ahead in the backwards walk.
pub(super) struct Provenance {
	/// The current historical identity.
	document: DocumentId,
	/// The sites for that identity, nearest lockfiles first.
	sites: Vec<Site>,
	/// Explicit rename boundaries, oldest first.
	links: Vec<Link>,
	/// Known projects used to read each historical site's rooted spellings.
	projects: std::collections::BTreeSet<RepoPath>,
	/// Whether a live or historical declaration supplied the chain already.
	adopted: bool,
	/// Whether the original query's batched blobs still apply.
	original: bool,
}

impl Provenance {
	/// Starts with the live chain, or learns a retired document's chain when
	/// its former live declaration first appears during the walk.
	pub(super) fn new(query: &Query) -> Result<Self, BackendError> {
		Ok(Self {
			document: query.document.clone(),
			sites: query.sites.clone(),
			links: query
				.renamed
				.iter()
				.map(|rename| Link::new(&query.project, rename))
				.collect::<Result<_, _>>()?,
			projects: query.projects.clone(),
			adopted: !query.renamed.is_empty(),
			original: true,
		})
	}

	/// The active historical identity.
	pub(super) fn document(&self) -> &DocumentId {
		&self.document
	}

	/// Its possible declaring files.
	pub(super) fn sites(&self) -> &[Site] {
		&self.sites
	}

	/// Whether the query's original batched blob view can be used.
	pub(super) fn original(&self) -> bool {
		self.original
	}

	/// The former identity's declaring paths for an additional batched read.
	pub(super) fn paths(&self) -> Vec<RepoPath> {
		self.sites.iter().map(|site| site.path.clone()).collect()
	}

	/// Crosses a boundary only before its marker existed at the active path.
	/// Unreadable authoritative declarations never establish that boundary.
	/// Each successful call consumes one link, including when several renames
	/// were committed together or a chain returns to an earlier spelling.
	pub(super) fn follow(
		&mut self,
		evidence: &Evidence,
		required: usize,
		obligation: bool,
	) -> Result<bool, BackendError> {
		let recorded = &evidence.markers;
		// A retired incarnation may share its marker with a later live one.
		// Learn a chain only where this query actually names that live marker.
		let live =
			evidence.live_occurrences > 0 && evidence.occurrences == required;
		if !self.adopted && !recorded.is_empty() && (obligation || live) {
			self.links = evidence
				.markers
				.iter()
				.map(|link| Link {
					from: link.from.clone(),
					at: link.at,
					by: link.by.clone(),
				})
				.collect();
			self.adopted = true;
			return Ok(false);
		}
		if evidence.unreadable || recorded.starts_with(&self.links) {
			return Ok(false);
		}
		let Some(link) = self.links.pop() else {
			return Ok(false);
		};
		let path = link.from;
		self.document = DocumentId::Local(path.clone());
		self.original = false;
		self.sites = vec![Site {
			project: crate::config::project_of(&self.projects, &path),
			path: path.clone(),
			kind: SiteKind::Frontmatter,
			former: false,
		}];
		let mut directory = path.parent();
		while let Some(current) = directory {
			let lockfile = current.join(crate::LOCKFILE_NAME).map_err(|error| {
				BackendError::new(BackendErrorKind::Io, error.to_string())
			})?;
			self.sites.push(Site {
				project: crate::config::project_of(&self.projects, &lockfile),
				path: lockfile,
				kind: SiteKind::LocalLockfile {
					directory: current.clone(),
				},
				former: true,
			});
			directory = current.parent();
		}
		Ok(true)
	}
}
