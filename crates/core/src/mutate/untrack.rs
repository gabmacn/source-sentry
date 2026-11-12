//! `untrack`: removing declared sources from a document's declaration.

use super::edits::Edits;
use super::repo::{Placement, Repo};
use super::track::SourceSpec;
use super::verify::verify;
use super::{MutateError, Planned};
use crate::claims::{DiagnosticKind, Inventory, Location};
use crate::model::{
	Declaration, DocumentId, RepoPath, SourceDecl, resolve_project_source,
};
use crate::ports::Snapshot;

/// What to remove.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UntrackRequest {
	/// The document whose declaration loses sources.
	pub document: DocumentId,
	/// The sources to remove, written as they would be given to `track`: a
	/// path matches the declared path that resolves to the same file, and a
	/// pattern matches the pattern written alike.
	pub sources: Vec<SourceSpec>,
}

/// What `untrack` changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UntrackSummary {
	/// The document.
	pub document: DocumentId,
	/// Where its declaration lives.
	pub placement: Placement,
	/// The file and field of the declaration.
	pub declared_in: Location,
	/// The sources removed, with the attestations they carried.
	pub removed: Vec<SourceDecl>,
	/// The declaration that remains.
	pub declaration: Declaration,
}

/// The errors about the document that removing sources can resolve: a
/// declared source that is missing or invalid, and the cycles a source
/// creates.
const TOLERATED: &[DiagnosticKind] = &[
	DiagnosticKind::MissingSource,
	DiagnosticKind::InvalidSourcePath,
	DiagnosticKind::InvalidPattern,
	DiagnosticKind::SelfDependency,
	DiagnosticKind::DependencyCycle,
];

/// Plans removing sources from a document's declaration, where it is. The
/// remaining sources keep their attestations, and the document keeps its
/// review. Removing a whole document is `retire`'s job, or deleting its
/// file's frontmatter key.
///
/// # Errors
///
/// Refuses, planning nothing:
///
/// - an untracked or doubly claimed document;
/// - a sealed record ([`MutateError::Sealed`]), whose sources are frozen;
/// - a document with configuration or integrity errors other than those
///   removing a source can resolve;
/// - a source that the declaration does not have
///   ([`MutateError::SourceNotDeclared`]);
/// - a change that would introduce another configuration or integrity
///   error.
///
/// Fails when the backend fails.
pub fn untrack<S: Snapshot>(
	snapshot: &S,
	inventory: &Inventory,
	request: &UntrackRequest,
) -> Result<Planned<UntrackSummary>, MutateError> {
	let repo = Repo::new(snapshot, inventory)?;
	let claim = repo.claim(&request.document)?;
	repo.refuse_errors(&claim.document, TOLERATED)?;
	if claim.is_sealed() {
		return Err(MutateError::Sealed {
			document: claim.document.clone(),
		});
	}
	let mut declaration = claim.declaration.clone();
	let mut removed = Vec::with_capacity(request.sources.len());
	for spec in &request.sources {
		let position = declaration
			.sources
			.iter()
			.position(|source| {
				names(&inventory.project, &claim.document, source, spec)
			})
			.ok_or_else(|| MutateError::SourceNotDeclared {
				document: claim.document.clone(),
				declared: match spec {
					SourceSpec::Path(path) | SourceSpec::Pattern(path) => path.clone(),
				},
			})?;
		removed.push(declaration.sources.remove(position));
	}
	let placement = Placement::of(claim);
	let mut edits = Edits::default();
	edits.place(
		&repo,
		&claim.document,
		&placement,
		Some(declaration.clone()),
	)?;
	let plan = edits.finish(&repo)?;
	verify(&repo, &plan)?;
	Ok(Planned {
		plan,
		summary: UntrackSummary {
			document: claim.document.clone(),
			placement,
			declared_in: claim.location(),
			removed,
			declaration,
		},
	})
}

/// Whether `spec` names the declared `source` of `document`.
fn names(
	project: &RepoPath,
	document: &DocumentId,
	source: &SourceDecl,
	spec: &SourceSpec,
) -> bool {
	let resolve = |declared: &str| {
		resolve_project_source(
			project,
			match document {
				DocumentId::Local(path) => Some(path),
				DocumentId::Remote(_) => None,
			},
			declared,
		)
		.ok()
	};
	match (source, spec) {
		(SourceDecl::Path(source), SourceSpec::Path(spec)) => {
			resolve(spec).is_some() && resolve(&source.path) == resolve(spec)
		}
		(SourceDecl::Pattern(source), SourceSpec::Pattern(spec)) => {
			&source.pattern == spec
		}
		_ => false,
	}
}
