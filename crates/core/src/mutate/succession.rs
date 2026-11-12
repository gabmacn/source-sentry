//! Declaration inheritance when a series entry is registered.

use super::repo::Repo;
use super::{MutateError, SourceSpec};
use crate::model::{
	Declaration, DocumentId, RepoPath, SourceDecl, rebase_path, rebase_pattern,
};
use crate::ports::Snapshot;

/// What a new series entry inherits, before command-line additions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inherited {
	/// The newest earlier entry with a live declaration.
	pub predecessor: RepoPath,
	/// Its declared settings and sources, rebased and without review markers.
	pub declaration: Declaration,
}

/// Copies the predecessor's explicit declaration, if there is one.
///
/// Sealed records may supply settings and source identities, but no review,
/// attestation, seal or completion is copied. Inherited sources must satisfy
/// the same registration checks as sources given on the command line.
pub(super) fn inherit<S: Snapshot>(
	repo: &Repo<'_, S>,
	document: &DocumentId,
) -> Result<Option<Inherited>, MutateError> {
	let DocumentId::Local(path) = document else {
		return Ok(None);
	};
	let Some(predecessor) = repo.inventory.predecessor(path) else {
		return Ok(None);
	};
	let DocumentId::Local(previous) = &predecessor.document else {
		return Ok(None);
	};
	repo.claim(&predecessor.document)?;
	let from = previous.parent().unwrap_or(RepoPath::ROOT);
	let to = path.parent().unwrap_or(RepoPath::ROOT);
	let specs = predecessor
		.declaration
		.sources
		.iter()
		.map(|source| {
			let rebased = match source {
				SourceDecl::Path(source) => {
					rebase_path(&source.path, &from, &to).map(SourceSpec::Path)
				}
				SourceDecl::Pattern(source) => {
					rebase_pattern(&source.pattern, &from, &to).map(SourceSpec::Pattern)
				}
			};
			rebased.map_err(|error| MutateError::CannotInherit {
				document: document.clone(),
				predecessor: previous.clone(),
				declared: source.declared().to_owned(),
				error,
			})
		})
		.collect::<Result<Vec<_>, _>>()?;
	Ok(Some(Inherited {
		predecessor: previous.clone(),
		declaration: Declaration {
			classes: predecessor.declaration.classes.clone(),
			owners: predecessor.declaration.owners.clone(),
			schedule: predecessor.declaration.schedule.clone(),
			sources: super::track::sources(repo, document, &specs)?,
			..Declaration::default()
		},
	}))
}
