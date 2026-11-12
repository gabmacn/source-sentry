//! Command-line sources: re-expression for declarations and typed selection.

use std::path::Path;

use source_sentry_core::claims::{Claim, Inventory, SourceGlob};
use source_sentry_core::model::{
	DocumentId, RepoPath, SourceDecl, rebase_path, rebase_pattern,
	resolve_rooted, resolve_source,
};
use source_sentry_core::mutate::{
	Expectation, MutateError, Selection, SourceSpec,
};

use super::Workspace;
use crate::cli::ExpectationArg;
use crate::exit::Failure;

impl Workspace {
	/// Re-expresses a source typed in the current directory for `document`.
	pub(crate) fn source_spec(
		&self,
		document: &DocumentId,
		spec: &SourceSpec,
	) -> Result<SourceSpec, Failure> {
		let to = match document {
			DocumentId::Local(path) => path.parent().unwrap_or(RepoPath::ROOT),
			DocumentId::Remote(_) => RepoPath::ROOT,
		};
		let (declared, pattern) = match spec {
			SourceSpec::Path(path) => (path, false),
			SourceSpec::Pattern(pattern) => (pattern, true),
		};
		if declared.is_empty() {
			return Err(Failure::usage(if pattern {
				"the pattern is empty"
			} else {
				"the path is empty"
			}));
		}
		let rebased = if pattern {
			let first = declared
				.find(['*', '?', '[', ']', '{', '}'])
				.unwrap_or(declared.len());
			let split = declared[..first].rfind('/').map_or(0, |index| index + 1);
			let (prefix, suffix) = declared.split_at(split);
			let base = self.source_path(Path::new(if prefix.is_empty() {
				"."
			} else {
				prefix
			}))?;
			if declared.starts_with('/') {
				return Ok(SourceSpec::Pattern(declared.clone()));
			}
			rebase_pattern(if suffix.is_empty() { "." } else { suffix }, &base, &to)
				.map_err(|error| {
				Failure::usage(format!("cannot rebase pattern `{declared}`: {error}"))
			})?
		} else {
			let path = self.source_path(Path::new(declared))?;
			if declared.starts_with('/') {
				declared.clone()
			} else {
				rebase_path(path.as_str(), &RepoPath::ROOT, &to)
					.map_err(|error| Failure::usage(error.to_string()))?
			}
		};
		// A remote identity has no document directory for a relative spelling.
		let rebased = if matches!(document, DocumentId::Remote(_))
			&& !rebased.starts_with('/')
		{
			format!("/{rebased}")
		} else {
			rebased
		};
		Ok(if pattern {
			SourceSpec::Pattern(rebased)
		} else {
			SourceSpec::Path(rebased)
		})
	}

	/// Resolves each selector before handing the core an exact, typed
	/// declaration. A spelling that names both kinds is invalid usage.
	pub(crate) fn selection(
		&self,
		inventory: &Inventory,
		document: &DocumentId,
		names: &[String],
	) -> Result<Selection, Failure> {
		if names.is_empty() {
			return Ok(Selection::All);
		}
		let Some(claim) = inventory
			.claims
			.iter()
			.find(|claim| &claim.document == document)
		else {
			// Let the planner report untracked documents and malformed claims.
			return Ok(Selection::Sources(names.to_vec()));
		};
		let sources = names
			.iter()
			.map(|name| self.selected_source(claim, name))
			.collect::<Result<_, _>>()?;
		Ok(Selection::Declared(sources))
	}

	/// Selects one path by its resolved file or one pattern by its spelling.
	fn selected_source(
		&self,
		claim: &Claim,
		name: &str,
	) -> Result<SourceSpec, Failure> {
		let resolved = self.source_path(Path::new(name));
		let path = claim
			.declaration
			.sources
			.iter()
			.find(|source| match source {
				SourceDecl::Path(source) => resolved.as_ref().is_ok_and(|path| {
					declared_path(&claim.document, &source.path).as_ref() == Some(path)
				}),
				SourceDecl::Pattern(_) => false,
			});
		let pattern =
			claim
				.declaration
				.sources
				.iter()
				.find(|source| match source {
					SourceDecl::Pattern(source) => {
						pattern_matches(&claim.document, &source.pattern, name)
					}
					SourceDecl::Path(_) => false,
				});
		match (path, pattern) {
			(Some(path), Some(pattern)) => Err(Failure::usage(format!(
				"`{name}` selects both path `{}` and pattern `{}`; use an unambiguous spelling",
				path.declared(),
				pattern.declared()
			))),
			(Some(source), None) => {
				Ok(SourceSpec::Path(source.declared().to_owned()))
			}
			(None, Some(source)) => {
				Ok(SourceSpec::Pattern(source.declared().to_owned()))
			}
			(None, None) => {
				resolved?;
				Err(Failure::mutate(
					self,
					&MutateError::UnknownSource {
						document: claim.document.clone(),
						selector: name.to_owned(),
						declared: claim
							.declaration
							.sources
							.iter()
							.map(|source| source.declared().to_owned())
							.collect(),
					},
				))
			}
		}
	}

	/// Converts typed removals without treating pattern spellings as paths.
	pub(crate) fn removals(
		&self,
		inventory: &Inventory,
		document: &DocumentId,
		specs: &[SourceSpec],
	) -> Result<Vec<SourceSpec>, Failure> {
		let claim = inventory
			.claims
			.iter()
			.find(|claim| &claim.document == document);
		specs
			.iter()
			.map(|spec| match spec {
				SourceSpec::Path(path) => self
					.source_path(Path::new(path))
					.map(|path| SourceSpec::Path(format!("/{}", path.as_str()))),
				SourceSpec::Pattern(pattern) => {
					let declared = claim
						.and_then(|claim| {
							claim
								.declaration
								.sources
								.iter()
								.find_map(|source| match source {
									SourceDecl::Pattern(source)
										if pattern_matches(document, &source.pattern, pattern) =>
									{
										Some(source.pattern.clone())
									}
									_ => None,
								})
						})
						.unwrap_or_else(|| pattern.clone());
					Ok(SourceSpec::Pattern(declared))
				}
			})
			.collect()
	}

	/// Resolves an expectation's command-line path after opening the workspace.
	pub(crate) fn expectation(
		&self,
		argument: &ExpectationArg,
	) -> Result<Expectation, Failure> {
		let path = self.source_path(&argument.path)?;
		if path.is_root() {
			return Err(Failure::usage(
				"an expectation must name a file, not the repository root",
			));
		}
		Ok(match &argument.oid {
			Some(oid) => Expectation::Present {
				path,
				oid: oid.clone(),
			},
			None => Expectation::Absent { path },
		})
	}

	/// An expectation with its file relative to the current directory.
	pub(crate) fn display_expectation(
		&self,
		expectation: &Expectation,
	) -> String {
		let path = self.display_path(expectation.path());
		match expectation {
			Expectation::Present { oid, .. } => format!("{path}@{oid}"),
			Expectation::Absent { .. } => format!("{path}@absent"),
		}
	}
}

/// The file named by a declared path, including missing sources.
fn declared_path(document: &DocumentId, declared: &str) -> Option<RepoPath> {
	match document {
		DocumentId::Local(path) => resolve_source(path, declared).ok(),
		DocumentId::Remote(_) => resolve_rooted(declared).ok(),
	}
}

/// A pattern's exact declaration or its normalized `/`-rooted form.
fn pattern_matches(document: &DocumentId, declared: &str, name: &str) -> bool {
	declared == name
		|| (name.starts_with('/')
			&& SourceGlob::compile(
				match document {
					DocumentId::Local(path) => Some(path),
					DocumentId::Remote(_) => None,
				},
				declared,
			)
			.is_ok_and(|glob| glob.rooted() == name))
}
