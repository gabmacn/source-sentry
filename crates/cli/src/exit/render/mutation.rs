//! Mutation refusals with every structured path displayed by the workspace.

use std::fmt;

use crate::describe;
use crate::workspace::Workspace;
use source_sentry_core::adapters::{Adapter, lockfile::LockfileError};
use source_sentry_core::model::RebaseError;
use source_sentry_core::mutate::{
	Expectation, Mismatch, MutateError, Placement, Target,
};
use source_sentry_core::ports::Canonical;

/// Renders every refusal explicitly, so new variants require a display decision.
#[expect(
	clippy::too_many_lines,
	reason = "one exhaustive error match makes every structured path conversion visible and rejects unhandled new variants"
)]
pub(super) fn render(workspace: &Workspace, error: &MutateError) -> String {
	match error {
		MutateError::OutsideProject {
			document,
			project,
			destination,
		} => {
			let document = workspace.display_document(document);
			let project = workspace.display_path(project);
			let destination = workspace.display_path(destination);
			format!(
				"`{document}` belongs to project {project}, so its declaration cannot be placed in {destination}. To move the document, use `git mv` followed by `source-sentry rename`"
			)
		}
		MutateError::DestinationClass { class, project } => {
			let project = workspace.display_path(project);
			format!(
				"destination project {project} does not define class `{class}`; declare or import the class before moving the declaration"
			)
		}
		MutateError::RenameRefused { old, new, reason } => {
			let old = workspace.display_document(old);
			let new = workspace.display_document(new);
			format!("cannot rename `{old}` to `{new}`: {reason}")
		}
		MutateError::CannotRenameSource {
			document,
			declared,
			error,
		} => {
			let document = workspace.display_document(document);
			let error = rebase(workspace, error);
			format!(
				"cannot rename `{document}`: cannot re-express source `{declared}`: {error}"
			)
		}
		MutateError::RenameMemberConflict {
			document,
			pattern,
			path,
		} => {
			let document = workspace.display_document(document);
			let path = workspace.display_path(path);
			format!(
				"the pattern `{pattern}` of `{document}` already reviews `{path}`. Run `diff` and review the pattern's membership before renaming; neither review was discarded"
			)
		}
		MutateError::CannotInherit {
			document,
			predecessor,
			declared,
			error,
		} => {
			let document = workspace.display_document(document);
			let predecessor = workspace.display_path(predecessor);
			let error = rebase(workspace, error);
			format!(
				"cannot register `{document}`: cannot inherit source `{declared}` from `{predecessor}`: {error}"
			)
		}
		MutateError::NotTracked { document, retired } => {
			let document = workspace.display_document(document);
			format!("`{document}` is not tracked. {}", not_tracked(*retired))
		}
		MutateError::DoubleClaim { document, sites } => {
			let document = workspace.display_document(document);
			let sites: Vec<_> = sites
				.iter()
				.map(|site| describe::location(workspace, site))
				.collect();
			format!(
				"`{document}` is claimed {} times ({}). Keep one declaration and remove \
		 the others first",
				sites.len(),
				list(&sites)
			)
		}
		MutateError::DocumentHasErrors {
			document,
			diagnostics,
		} => {
			let document = workspace.display_document(document);
			format!(
				"`{document}` has errors to fix first:\n{}",
				super::diagnostics(workspace, diagnostics)
			)
		}
		MutateError::SourceNotDeclared { document, declared } => {
			let document = workspace.display_document(document);
			format!(
				"`{document}` declares no source `{declared}`; `explain` lists the 		 sources it declares"
			)
		}
		MutateError::Sealed { document } => {
			let document = workspace.display_document(document);
			format!(
				"`{document}` is a sealed record: its sources are frozen, and `attest` \
		 never seals or reseals. A correction is a new, superseding record"
			)
		}
		MutateError::AlreadySealed { document, at } => {
			let document = workspace.display_document(document);
			format!(
				"`{document}` was already sealed at {at}. A correction is a new, \
		 superseding record"
			)
		}
		MutateError::NotCanonical { path, canonical } => {
			let path = workspace.display_path(path);
			format!(
				"cannot canonicalize `{path}`: {}; no oid was guessed",
				canonical_problem(canonical)
			)
		}
		MutateError::NothingSelected { document } => {
			let document = workspace.display_document(document);
			format!(
				"the selection names no source of `{document}`; name at least one \
		 declared path or pattern, or select all of them"
			)
		}
		MutateError::UnknownSource {
			document,
			selector,
			declared,
		} => {
			let document = workspace.display_document(document);
			format!(
				"`{document}` declares no source `{selector}`; its sources are {}",
				quoted(declared)
			)
		}
		MutateError::MissingSource {
			document,
			declared,
			path,
		} => {
			let document = workspace.display_document(document);
			let path = workspace.display_path(path);
			format!(
				"the source `{declared}` of `{document}` is `{path}`, which is not \
		 tracked in this snapshot. Restore the file or correct the path; only \
		 a pattern's membership can acknowledge a removal"
			)
		}
		MutateError::LockfileSource { document, declared } => {
			let document = workspace.display_document(document);
			format!(
				"the source `{declared}` of `{document}` names a Source Sentry lockfile. \
		 A lockfile cannot be a source: recording a review rewrites it and \
		 makes the document stale again. Choose a source file instead"
			)
		}
		MutateError::EmptyPattern { document, pattern } => {
			let document = workspace.display_document(document);
			format!(
				"the new pattern `{pattern}` of `{document}` matches no tracked file, \
		 and a new pattern that matches nothing is never recorded. Correct the \
		 pattern, or add its files to git first"
			)
		}
		MutateError::SubmoduleSource {
			document,
			declared,
			path,
		} => {
			let document = workspace.display_document(document);
			let path = workspace.display_path(path);
			format!(
				"the source `{declared}` of `{document}` names `{path}`, which is a \
		 submodule. Source Sentry does not support submodules: name files \
		 outside it, or narrow the pattern to leave it out"
			)
		}
		MutateError::ExpectationMismatch {
			document,
			mismatches,
		} => {
			let document = workspace.display_document(document);
			let mismatches: Vec<_> = mismatches
				.iter()
				.map(|difference| mismatch(workspace, difference))
				.collect();
			format!(
				"the snapshot does not match the expectations for `{document}`:\n{}\nRun \
		 `diff` again, and have the changes reviewed",
				lines(&mismatches)
			)
		}
		MutateError::NothingToAttest { document } => {
			let document = workspace.display_document(document);
			format!(
				"nothing to attest: `{document}` has no source in the selection. Pass \
		 the reread option to record a review of the whole document"
			)
		}
		MutateError::RemoteRecord { document } => {
			let document = workspace.display_document(document);
			format!(
				"`{document}` is a remote document, and remote documents cannot be records"
			)
		}
		MutateError::OnFileRecord { document } => {
			let document = workspace.display_document(document);
			format!(
				"`{document}` is tracked on-file, but records are tracked through \
		 lockfiles only. Move it into a lockfile with `track --from on-file` \
		 first, then seal it"
			)
		}
		MutateError::RecordToOnFile { document } => {
			let document = workspace.display_document(document);
			format!(
				"`{document}` is a sealed record, and records are tracked through \
		 lockfiles only; move it to another lockfile instead"
			)
		}
		MutateError::SealLivingHead { document, series } => {
			let document = workspace.display_document(document);
			format!(
				"`{document}` is the living head of the series `{series}`, and a \
		 `living` series whose newest entry is sealed is an error. Add a \
		 successor first; this entry can be sealed once it is a predecessor"
			)
		}
		MutateError::RetireLivingHead {
			document,
			series,
			config,
		} => {
			let document = workspace.display_document(document);
			let config = config.as_ref().map(|path| workspace.display_path(path));
			format!(
				"`{document}` is the living head of the series `{series}`. Retiring it \
		 requires retiring the series, which is defined in {}: remove the \
		 series there first. A head created by mistake is replaced by adding a \
		 new successor",
				defined_in(config.as_ref())
			)
		}
		MutateError::OnFileRetire { document } => {
			let document = workspace.display_document(document);
			format!(
				"`{document}` is tracked on-file, so it needs no tombstone: deleting \
		 the file, or its `source-sentry` key, removes its claim. Only lockfile \
		 entries are retired"
			)
		}
		MutateError::AlreadyClaimed { document, sites } => {
			let document = workspace.display_document(document);
			let sites: Vec<_> = sites
				.iter()
				.map(|site| describe::location(workspace, site))
				.collect();
			format!(
				"`{document}` is already tracked ({}), and a second claim would be a \
		 double claim. Use `track --from` to move it",
				list(&sites)
			)
		}
		MutateError::NotADocument { path } => {
			let path = workspace.display_path(path);
			format!(
				"`{path}` is one of Source Sentry's own files (a file under \
		 `.source-sentry/`, a lockfile or a directory config), never a document"
			)
		}
		MutateError::DocumentNotFound { path } => {
			let path = workspace.display_path(path);
			format!("`{path}` is not tracked in this snapshot; add it to git first")
		}
		MutateError::NotDeclarationCapable { document, adapter } => {
			let document = workspace.display_document(document);
			format!(
				"`{document}` is read as {}, which cannot carry an on-file declaration; \
		 track it in a lockfile instead",
				adapter_name(*adapter)
			)
		}
		MutateError::UsedAsSource {
			document,
			dependents,
		} => {
			let document = workspace.display_document(document);
			let dependents: Vec<_> = dependents
				.iter()
				.map(|document| workspace.display_document(document))
				.collect();
			format!(
				"`{document}` is a source of {}, so it must be tracked through a \
		 lockfile: attesting an on-file document rewrites it, which would make \
		 every document that depends on it stale",
				quoted(&dependents)
			)
		}
		MutateError::TargetMismatch { document, target } => {
			let document = workspace.display_document(document);
			let target = self::target(workspace, target);
			format!(
				"`{document}` cannot be tracked {target}: local documents are tracked \
		 on-file or in a local lockfile, and remote documents in an owner \
		 lockfile"
			)
		}
		MutateError::InvalidSource {
			document,
			declared,
			error,
		} => {
			let document = workspace.display_document(document);
			format!("the source `{declared}` of `{document}` is invalid: {error}")
		}
		MutateError::DuplicateSource {
			document,
			first,
			declared,
			pattern,
		} => {
			let document = workspace.display_document(document);
			format!(
				"the sources `{first}` and `{declared}` of `{document}` name the same \
		 {}; declare it once",
				if *pattern { "pattern" } else { "file" }
			)
		}
		MutateError::SelfDependency { document, declared } => {
			let document = workspace.display_document(document);
			format!(
				"the source `{declared}` is `{document}` itself, and a document cannot \
		 be its own source"
			)
		}
		MutateError::UndeclaredClass { class } => {
			let directory =
				workspace.display_path(&source_sentry_core::config::project_of(
					&workspace.projects,
					&workspace.scope,
				));
			let config = if directory == "." {
				".source-sentry/source-sentry.yaml".to_owned()
			} else {
				format!("{directory}/.source-sentry/source-sentry.yaml")
			};
			format!(
				"class `{class}` is not declared. Declare it under `define.classes` in {config}"
			)
		}
		MutateError::UrlNotRemote { document } => {
			let document = workspace.display_document(document);
			format!(
				"`{document}` is a local document; only remote documents carry a `url`"
			)
		}
		MutateError::NotAnAncestor {
			document,
			directory,
		} => {
			let document = workspace.display_path(document);
			let directory = workspace.display_path(directory);
			format!(
				"`{directory}` does not contain `{document}`, and a lockfile lists only \
		 documents inside its directory"
			)
		}
		MutateError::NearerLockfile {
			document,
			directory,
			nearest,
		} => {
			let document = workspace.display_path(document);
			let directory = workspace.display_path(directory);
			let nearest = workspace.display_path(nearest);
			format!(
				"{nearest} is nearer to `{document}` than `{directory}`, and only the \
		 nearest lockfile may list a document. Use that lockfile, or a \
		 directory below it"
			)
		}
		MutateError::StrandedEntries {
			lockfile,
			directory,
			documents,
		} => {
			let lockfile = workspace.display_path(lockfile);
			let directory = workspace.display_path(directory);
			let documents: Vec<_> = documents
				.iter()
				.map(|document| workspace.display_document(document))
				.collect();
			format!(
				"the lockfile with authority below `{directory}` changes, and {lockfile} \
		 would keep entries there: {}. Move them in the same `track --from`",
				quoted(&documents)
			)
		}
		MutateError::InvalidLockfile { path, error } => {
			let path = workspace.display_path(path);
			let error = lockfile(workspace, error);
			format!("{path} cannot be used: {error}. Correct it first")
		}
		MutateError::InvalidFrontmatter { document, error } => {
			let document = workspace.display_path(document);
			format!("the frontmatter of `{document}` cannot be rewritten: {error}")
		}
		MutateError::RoundTrip { path } => {
			let path = workspace.display_path(path);
			format!(
				"{path} would not read back as planned, so nothing was planned; this \
		 is a bug in Source Sentry"
			)
		}
		MutateError::WrongOrigin {
			document,
			expected,
			actual,
		} => {
			let document = workspace.display_document(document);
			let expected = placement(workspace, expected);
			let actual = placement(workspace, actual);
			format!("`{document}` is tracked {actual}, not {expected}")
		}
		MutateError::SamePlacement {
			document,
			placement,
		} => {
			let document = workspace.display_document(document);
			let placement = self::placement(workspace, placement);
			format!("`{document}` is already tracked {placement}")
		}
		MutateError::RepeatedDocument { document } => {
			let document = workspace.display_document(document);
			format!("`{document}` is named more than once")
		}
		MutateError::TombstoneConflict { document, from, to } => {
			let document = workspace.display_document(document);
			let from = workspace.display_path(from);
			let to = workspace.display_path(to);
			format!(
				"`{document}` has tombstones in both {from} and {to}; resolve one of \
		 them in review first"
			)
		}
		MutateError::WouldIntroduce { diagnostics } => {
			format!(
				"the change would introduce these errors, so nothing was planned:\n{}",
				super::diagnostics(workspace, diagnostics)
			)
		}
		MutateError::InvalidConfigs { errors } => {
			format!(
				"the configs would no longer load after the change, so nothing was \
				 planned:\n{}",
				lines(
					&errors
						.iter()
						.map(|error| super::config(workspace, error))
						.collect::<Vec<_>>()
				)
			)
		}
		MutateError::NotACandidate { path } => {
			let path = workspace.display_path(path);
			format!(
				"`{path}` is not an unclaimed candidate; `init` registers only files \
		 that a root's scan finds and nothing claims"
			)
		}
		MutateError::UnreadableSchema { path, reason } => {
			let path = workspace.display_path(path);
			format!("{path} has no readable schema: {reason}. Correct it first")
		}
		MutateError::UnknownSchema {
			path,
			found,
			current,
		} => {
			let path = workspace.display_path(path);
			format!(
				"{path} uses lockfile schema {found}, which this version of Source \
		 Sentry does not know; it reads schema {current}. Use a version of \
		 Source Sentry that supports it"
			)
		}
		MutateError::Backend(_)
		| MutateError::EmptyOwner
		| MutateError::NothingToMove
		| MutateError::InvalidOwnerName { .. }
		| MutateError::InvalidReviewer { .. }
		| MutateError::InvalidSchedule(_) => super::super::chain(error),
	}
}

/// A resolved placement with its directory or project displayed locally.
fn placement(workspace: &Workspace, value: &Placement) -> String {
	match value {
		Placement::OnFile => "on-file".to_owned(),
		Placement::Lockfile { directory } => {
			let directory = workspace.display_path(directory);
			if directory == "." {
				"in source-sentry.lock".to_owned()
			} else {
				format!("in {directory}/source-sentry.lock")
			}
		}
		Placement::Remote { project, owner } => format!(
			"in the owner lockfile of `{owner}` in project {}",
			workspace.display_path(project)
		),
	}
}

/// A requested target, retaining the nearest-lockfile default description.
fn target(workspace: &Workspace, value: &Target) -> String {
	match value {
		Target::OnFile => placement(workspace, &Placement::OnFile),
		Target::Lockfile { directory: None } => "in a local lockfile".to_owned(),
		Target::Lockfile {
			directory: Some(directory),
		} => placement(
			workspace,
			&Placement::Lockfile {
				directory: directory.clone(),
			},
		),
		Target::Remote { project, owner } => placement(
			workspace,
			&Placement::Remote {
				project: project.clone(),
				owner: owner.clone(),
			},
		),
	}
}

/// A bound expectation with its file displayed from the effective directory.
fn expectation(workspace: &Workspace, value: &Expectation) -> String {
	match value {
		Expectation::Present { path, oid } => {
			format!("{}@{oid}", workspace.display_path(path))
		}
		Expectation::Absent { path } => {
			format!("{}@absent", workspace.display_path(path))
		}
	}
}

/// One nested mismatch without losing the path inside an expectation.
fn mismatch(workspace: &Workspace, value: &Mismatch) -> String {
	match value {
		Mismatch::Unexpected(actual) => format!(
			"`{}` is in the snapshot, but no expectation names it",
			expectation(workspace, actual)
		),
		Mismatch::Differs {
			expected,
			actual: Some(actual),
		} => format!(
			"expected `{}`, found `{}`",
			expectation(workspace, expected),
			expectation(workspace, actual)
		),
		Mismatch::Differs {
			expected,
			actual: None,
		} => format!(
			"expected `{}`, but `{}` is not part of the selection",
			expectation(workspace, expected),
			workspace.display_path(expected.path())
		),
		Mismatch::Repeated(path) => {
			format!("`{}` is named more than once", workspace.display_path(path))
		}
	}
}

/// A re-expression error with its structured directory displayed locally.
fn rebase(workspace: &Workspace, value: &RebaseError) -> String {
	match value {
		RebaseError::PatternDirectory(path) => format!(
			"the directory `{}` contains glob metacharacters",
			workspace.display_path(path)
		),
		other => other.to_string(),
	}
}

/// Lockfile directory constraints carry paths independently of declaration spellings.
fn lockfile(workspace: &Workspace, value: &LockfileError) -> String {
	match value {
		LockfileError::OutsideDirectory { path, directory } => format!(
			"`{}` is not inside the lockfile's directory `{}`",
			workspace.display_path(path),
			workspace.display_path(directory)
		),
		other => other.to_string(),
	}
}

/// The advice for an untracked document.
fn not_tracked(retired: bool) -> &'static str {
	if retired {
		"It was retired; `track` registers a new document under the same name"
	} else {
		"Register it with `track` first"
	}
}

/// What a non-oid canonical value means, and how to fix it.
fn canonical_problem(canonical: &Canonical) -> String {
	match canonical {
		Canonical::Oid(oid) => format!("it has the oid {oid}"),
		Canonical::UnsupportedFilter {
			filter,
			attribute_origin,
		} => format!(
			"unsupported clean filter {filter} ({attribute_origin}). Evaluate \
			 committed blobs with --rev, or use a supported conversion"
		),
		Canonical::UnsupportedLfsExtension => {
			"its Git LFS pointer uses extensions, which are not supported".to_owned()
		}
		Canonical::CrlfInIndex => "the index holds it with CRLF line endings \
		                           although its attributes ask git to normalize \
		                           it. Run `git add --renormalize .`"
			.to_owned(),
		Canonical::Submodule => "it is a submodule, and submodules are not \
		                         supported"
			.to_owned(),
	}
}

/// Where a series is defined.
fn defined_in(config: Option<&String>) -> String {
	config.map_or_else(|| "the built-in defaults".to_owned(), ToString::to_string)
}

/// An adapter's name, or what it means to have none.
fn adapter_name(adapter: Option<Adapter>) -> &'static str {
	adapter.map_or("no adapter", Adapter::name)
}

/// Items in backticks, separated by commas.
fn quoted<T: fmt::Display>(items: &[T]) -> String {
	items
		.iter()
		.map(|item| format!("`{item}`"))
		.collect::<Vec<_>>()
		.join(", ")
}

/// Items separated by commas.
fn list<T: fmt::Display>(items: &[T]) -> String {
	items
		.iter()
		.map(ToString::to_string)
		.collect::<Vec<_>>()
		.join(", ")
}

/// Items on lines of their own, each indented and bulleted.
fn lines<T: fmt::Display>(items: &[T]) -> String {
	items
		.iter()
		.map(|item| format!("  - {item}"))
		.collect::<Vec<_>>()
		.join("\n")
}
