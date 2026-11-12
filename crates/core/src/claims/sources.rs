//! Resolving each claim's sources against the snapshot.

use std::collections::{BTreeMap, BTreeSet};

use super::pattern::SourceGlob;
use super::{
	Attestation, Claim, Diagnostic, DiagnosticKind, ResolvedPath,
	ResolvedPattern, ResolvedSource,
};
use crate::model::{
	DocumentId, PathError, PathSource, PatternSource, RepoPath, SourceDecl,
	resolve_project_source,
};

/// Resolves the sources of every claim, and reports what is wrong with them.
///
/// For each claim:
///
/// - path sources resolve with [`resolve_source`] for a local document and
///   [`resolve_rooted`] for a remote one;
/// - patterns resolve as [`SourceGlob`] describes, and a live obligation's
///   patterns expand over `tracked`;
/// - attested pattern members resolve as path sources do.
///
/// A sealed record's sources are frozen: its patterns are not
/// expanded, and its missing sources and empty patterns are not reported.
pub(super) fn resolve(
	claims: &mut [Claim],
	tracked: &BTreeSet<RepoPath>,
	project: &RepoPath,
	diagnostics: &mut Vec<Diagnostic>,
) {
	for claim in claims {
		let sources = Resolver {
			claim,
			project,
			tracked,
			diagnostics: &mut *diagnostics,
		}
		.sources();
		claim.sources = sources;
	}
}

/// Resolves one claim's sources.
struct Resolver<'a> {
	project: &'a RepoPath,
	claim: &'a Claim,
	tracked: &'a BTreeSet<RepoPath>,
	diagnostics: &'a mut Vec<Diagnostic>,
}

impl Resolver<'_> {
	/// The local document's path, or `None` for a remote document.
	fn document(&self) -> Option<&RepoPath> {
		match &self.claim.document {
			DocumentId::Local(path) => Some(path),
			DocumentId::Remote(_) => None,
		}
	}

	/// Every source that resolves, in declared order.
	fn sources(&mut self) -> Vec<ResolvedSource> {
		let mut resolved = Vec::new();
		for (index, source) in self.claim.declaration.sources.iter().enumerate() {
			let source = match source {
				SourceDecl::Path(source) => self.path(index, source),
				SourceDecl::Pattern(source) => self.pattern(index, source),
			};
			resolved.extend(source);
		}
		resolved
	}

	/// The field of the source at `index`.
	fn field(&self, index: usize) -> String {
		format!("{}.sources[{index}]", self.claim.field)
	}

	/// Resolves a declared path.
	fn resolve(&self, declared: &str) -> Result<RepoPath, PathError> {
		resolve_project_source(self.project, self.document(), declared)
	}

	/// Resolves a path source.
	fn path(
		&mut self,
		index: usize,
		source: &PathSource,
	) -> Option<ResolvedSource> {
		let field = format!("{}.path", self.field(index));
		let path = match self.resolve(&source.path) {
			Ok(path) => path,
			Err(error) => {
				self.report(
					DiagnosticKind::InvalidSourcePath,
					field,
					format!("{error}. Correct the source's path"),
				);
				return None;
			}
		};
		if path.is_lockfile() {
			self.report(
				DiagnosticKind::LockfileSource,
				field,
				format!(
					"`{path}` is a Source Sentry lockfile, and cannot be a source: \
					 recording a review rewrites it and makes the document stale \
					 again. Remove this source"
				),
			);
			return None;
		}
		if self.document() == Some(&path) {
			self.report_self_dependency(field.clone(), &source.path);
		}
		let exists = self.tracked.contains(&path);
		if !exists && !self.claim.is_sealed() {
			self.report(
				DiagnosticKind::MissingSource,
				field,
				format!(
					"the source `{path}` is not tracked in this snapshot. Restore it, \
					 correct the path, or remove the source; a pattern follows files \
					 that come and go"
				),
			);
		}
		let attestation = match (&source.oid, &source.reviewed) {
			(Some(oid), Some(reviewed)) => Some(Attestation {
				oid: oid.clone(),
				reviewed: reviewed.clone(),
			}),
			_ => None,
		};
		Some(ResolvedSource::Path(ResolvedPath {
			index,
			path,
			exists,
			attestation,
		}))
	}

	/// Resolves a pattern source, its attested members and, for a live
	/// obligation, its current matches.
	fn pattern(
		&mut self,
		index: usize,
		source: &PatternSource,
	) -> Option<ResolvedSource> {
		let field = format!("{}.pattern", self.field(index));
		let glob = match SourceGlob::compile_project(
			self.project,
			self.document(),
			&source.pattern,
		) {
			Ok(glob) => glob,
			Err(problem) => {
				self.report(
					DiagnosticKind::InvalidPattern,
					field,
					format!(
						"`{}` is not a valid source pattern: {problem}",
						source.pattern
					),
				);
				return None;
			}
		};
		let members = self.members(index, source);
		let matches = if self.claim.is_sealed() {
			None
		} else {
			let matches = glob.expand(self.tracked);
			if matches.is_empty() {
				self.report(
					DiagnosticKind::EmptyPattern,
					field.clone(),
					format!(
						"`{}` matches no tracked file. Correct the pattern, or remove it \
						 once its members are gone for good",
						source.pattern
					),
				);
			}
			if self
				.document()
				.is_some_and(|document| matches.contains(document))
			{
				self.report_self_dependency(field, &source.pattern);
			}
			Some(matches)
		};
		Some(ResolvedSource::Pattern(ResolvedPattern {
			index,
			glob: glob.rooted(),
			matches,
			members,
		}))
	}

	/// The attested members of a pattern, by canonical path.
	fn members(
		&mut self,
		index: usize,
		source: &PatternSource,
	) -> BTreeMap<RepoPath, Attestation> {
		let mut members = BTreeMap::new();
		for (position, member) in source.members.iter().enumerate() {
			match self.resolve(&member.path) {
				Ok(path) if path.is_lockfile() => {}
				Ok(path) => {
					members.insert(
						path,
						Attestation {
							oid: member.oid.clone(),
							reviewed: member.reviewed.clone(),
						},
					);
				}
				Err(error) => self.report(
					DiagnosticKind::InvalidSourcePath,
					format!("{}.members[{position}].path", self.field(index)),
					format!("{error}. Correct the member's path"),
				),
			}
		}
		members
	}

	/// Reports a source that resolves to the document itself.
	fn report_self_dependency(&mut self, field: String, declared: &str) {
		self.report(
			DiagnosticKind::SelfDependency,
			field,
			format!(
				"`{declared}` resolves to `{}` itself, and a document cannot be its \
				 own source. Remove the source, or narrow the pattern",
				self.claim.document
			),
		);
	}

	/// Reports a problem with a field of the claim's declaration.
	fn report(&mut self, kind: DiagnosticKind, field: String, message: String) {
		let location = self.claim.location_of(field);
		self.diagnostics.push(
			Diagnostic::new(kind, location, message)
				.about(self.claim.document.clone()),
		);
	}
}
