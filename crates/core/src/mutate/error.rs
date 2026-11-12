//! Why a mutation command refuses.
//!
//! Every refusal names what to fix. None of them is a partial result: a
//! refused command plans nothing.

use std::fmt;

use chrono::{DateTime, Utc};

use super::{Expectation, Placement};
use crate::adapters::Adapter;
use crate::adapters::frontmatter::FrontmatterError;
use crate::adapters::lockfile::LockfileError;
use crate::claims::{Diagnostic, Location};
use crate::config::ConfigError;
use crate::evaluate::schedule::ScheduleError;
use crate::model::{ClassName, DocumentId, PathError, RepoPath};
use crate::ports::{BackendError, Canonical};

/// Why a mutation command planned nothing.
#[derive(Debug, thiserror::Error)]
pub enum MutateError {
	/// The repository could not be read, so the command could not complete
	/// (exit 4).
	#[error(transparent)]
	Backend(#[from] BackendError),
	/// The document has no claim.
	#[error("`{document}` is not tracked. {}", not_tracked(*.retired))]
	NotTracked {
		/// The document.
		document: DocumentId,
		/// Whether a tombstone names it.
		retired: bool,
	},
	/// The document has more than one claim.
	#[error(
		"`{document}` is claimed {} times ({}). Keep one declaration and remove \
		 the others first",
		.sites.len(),
		list(.sites)
	)]
	DoubleClaim {
		/// The document.
		document: DocumentId,
		/// Every declaration of it.
		sites: Vec<Location>,
	},
	/// Discovery reports configuration or integrity errors about the
	/// document, which must be fixed first.
	#[error("`{document}` has errors to fix first:\n{}", lines(.diagnostics))]
	DocumentHasErrors {
		/// The document.
		document: DocumentId,
		/// The errors.
		diagnostics: Vec<Diagnostic>,
	},
	/// `untrack` named a source that the declaration does not have.
	#[error(
		"`{document}` declares no source `{declared}`; `explain` lists the 		 sources it declares"
	)]
	SourceNotDeclared {
		/// The document.
		document: DocumentId,
		/// The source as given.
		declared: String,
	},
	/// The document is a sealed record, whose sources are frozen.
	#[error(
		"`{document}` is a sealed record: its sources are frozen, and `attest` \
		 never seals or reseals. A correction is a new, superseding record"
	)]
	Sealed {
		/// The record.
		document: DocumentId,
	},
	/// `seal` on a record that is already sealed.
	#[error(
		"`{document}` was already sealed at {at}. A correction is a new, \
		 superseding record"
	)]
	AlreadySealed {
		/// The record.
		document: DocumentId,
		/// When it was sealed.
		at: DateTime<Utc>,
	},
	/// A file's canonical oid could not be computed, so none was guessed.
	#[error(
		"cannot canonicalize `{path}`: {}; no oid was guessed",
		canonical_problem(.canonical)
	)]
	NotCanonical {
		/// The file.
		path: RepoPath,
		/// What the backend reported instead of an oid.
		canonical: Canonical,
	},
	/// An explicit selection that names no source.
	#[error(
		"the selection names no source of `{document}`; name at least one \
		 declared path or pattern, or select all of them"
	)]
	NothingSelected {
		/// The document.
		document: DocumentId,
	},
	/// A selector that matches none of the document's sources.
	#[error(
		"`{document}` declares no source `{selector}`; its sources are {}",
		quoted(.declared)
	)]
	UnknownSource {
		/// The document.
		document: DocumentId,
		/// The selector as given.
		selector: String,
		/// The document's sources, as declared.
		declared: Vec<String>,
	},
	/// An explicitly named source that the snapshot lacks.
	#[error(
		"the source `{declared}` of `{document}` is `{path}`, which is not \
		 tracked in this snapshot. Restore the file or correct the path; only \
		 a pattern's membership can acknowledge a removal"
	)]
	MissingSource {
		/// The document.
		document: DocumentId,
		/// The source, as declared.
		declared: String,
		/// The path it resolves to.
		path: RepoPath,
	},
	/// A new pattern that matches nothing.
	#[error(
		"the new pattern `{pattern}` of `{document}` matches no tracked file, \
		 and a pattern that matches nothing is never recorded. Correct the \
		 pattern, or add its files to git first"
	)]
	EmptyPattern {
		/// The document.
		document: DocumentId,
		/// The pattern, as declared.
		pattern: String,
	},
	/// A new source names a submodule, which has no content to review.
	#[error(
		"the source `{declared}` of `{document}` names `{path}`, which is a \
		 submodule. Source Sentry does not support submodules: name files \
		 outside it, or narrow the pattern to leave it out"
	)]
	SubmoduleSource {
		/// The document.
		document: DocumentId,
		/// The path or pattern, as declared.
		declared: String,
		/// The submodule.
		path: RepoPath,
	},
	/// The snapshot differs from the expectations the reviewer approved.
	#[error(
		"the snapshot does not match the expectations for `{document}`:\n{}\nRun \
		 `diff` again, and have the changes reviewed",
		lines(.mismatches)
	)]
	ExpectationMismatch {
		/// The document.
		document: DocumentId,
		/// Every difference, sorted by path.
		mismatches: Vec<Mismatch>,
	},
	/// `attest` with no source in the selection and no reread.
	#[error(
		"nothing to attest: `{document}` has no source in the selection. Pass \
		 the reread option to record a review of the whole document"
	)]
	NothingToAttest {
		/// The document.
		document: DocumentId,
	},
	/// `seal` on a remote document.
	#[error(
		"`{document}` is a remote document, and remote documents cannot be records"
	)]
	RemoteRecord {
		/// The document.
		document: DocumentId,
	},
	/// `seal` on an on-file document.
	#[error(
		"`{document}` is tracked on-file, but records are tracked through \
		 lockfiles only. Move it into a lockfile with `track --from on-file` \
		 first, then seal it"
	)]
	OnFileRecord {
		/// The document.
		document: DocumentId,
	},
	/// `track --from` that would move a record on-file.
	#[error(
		"`{document}` is a sealed record, and records are tracked through \
		 lockfiles only; move it to another lockfile instead"
	)]
	RecordToOnFile {
		/// The record.
		document: DocumentId,
	},
	/// `seal` on the living head of a series.
	#[error(
		"`{document}` is the living head of the series `{series}`, and a \
		 `living` series whose newest entry is sealed is an error. Add a \
		 successor first; this entry can be sealed once it is a predecessor"
	)]
	SealLivingHead {
		/// The head.
		document: DocumentId,
		/// The series' name.
		series: String,
	},
	/// `retire` on the living head of a series.
	#[error(
		"`{document}` is the living head of the series `{series}`. Retiring it \
		 requires retiring the series, which is defined in {}: remove the \
		 series there first. A head created by mistake is replaced by adding a \
		 new successor",
		defined_in(.config.as_ref())
	)]
	RetireLivingHead {
		/// The head.
		document: DocumentId,
		/// The series' name.
		series: String,
		/// The config that defines the series.
		config: Option<RepoPath>,
	},
	/// `retire` on an on-file document.
	#[error(
		"`{document}` is tracked on-file, so it needs no tombstone: deleting \
		 the file, or its `source-sentry` key, removes its claim. Only lockfile \
		 entries are retired"
	)]
	OnFileRetire {
		/// The document.
		document: DocumentId,
	},
	/// A tombstone already exists where a new one would go.
	#[error(
		"{lockfile} already holds a tombstone for `{document}`, from an earlier \
		 document at the same key, and Source Sentry never replaces a \
		 tombstone. Resolve the earlier tombstone in review first"
	)]
	AlreadyRetired {
		/// The document.
		document: DocumentId,
		/// The lockfile holding the tombstone.
		lockfile: RepoPath,
	},
	/// `track` on a document that is already claimed.
	#[error(
		"`{document}` is already tracked ({}), and a second claim would be a \
		 double claim. Use `track --from` to move it",
		list(.sites)
	)]
	AlreadyClaimed {
		/// The document.
		document: DocumentId,
		/// Its declarations.
		sites: Vec<Location>,
	},
	/// A path that is one of Source Sentry's own files.
	#[error(
		"`{path}` is one of Source Sentry's own files (a file under \
		 `.source-sentry/`, a lockfile or a directory config), never a document"
	)]
	NotADocument {
		/// The path.
		path: RepoPath,
	},
	/// A local document that the snapshot lacks.
	#[error("`{path}` is not tracked in this snapshot; add it to git first")]
	DocumentNotFound {
		/// The path.
		path: RepoPath,
	},
	/// On-file tracking of a document whose adapter cannot carry a
	/// declaration.
	#[error(
		"`{document}` is read as {}, which cannot carry an on-file declaration; \
		 track it in a lockfile instead",
		adapter_name(*.adapter)
	)]
	NotDeclarationCapable {
		/// The document.
		document: DocumentId,
		/// Its adapter, or `None` when no mapping matches it.
		adapter: Option<Adapter>,
	},
	/// On-file tracking of a document that is a source of another document.
	#[error(
		"`{document}` is a source of {}, so it must be tracked through a \
		 lockfile: attesting an on-file document rewrites it, which would make \
		 every document that depends on it stale",
		quoted(.dependents)
	)]
	UsedAsSource {
		/// The document.
		document: DocumentId,
		/// The documents that list it as a source.
		dependents: Vec<DocumentId>,
	},
	/// A target that does not fit the document's kind.
	#[error(
		"`{document}` cannot be tracked {target}: local documents are tracked \
		 on-file or in a local lockfile, and remote documents in an owner \
		 lockfile"
	)]
	TargetMismatch {
		/// The document.
		document: DocumentId,
		/// The target, described.
		target: String,
	},
	/// A source path or pattern that cannot be resolved.
	#[error("the source `{declared}` of `{document}` is invalid: {error}")]
	InvalidSource {
		/// The document.
		document: DocumentId,
		/// The source, as declared.
		declared: String,
		/// Why it was rejected.
		error: PathError,
	},
	/// Two sources that name the same file, or the same pattern.
	#[error(
		"the sources `{first}` and `{declared}` of `{document}` name the same \
		 {}; declare it once",
		if *.pattern { "pattern" } else { "file" }
	)]
	DuplicateSource {
		/// The document.
		document: DocumentId,
		/// The earlier source.
		first: String,
		/// The later source.
		declared: String,
		/// Whether both are patterns.
		pattern: bool,
	},
	/// A source that resolves to the document itself.
	#[error(
		"the source `{declared}` is `{document}` itself, and a document cannot \
		 be its own source"
	)]
	SelfDependency {
		/// The document.
		document: DocumentId,
		/// The source, as declared.
		declared: String,
	},
	/// A class that the repository config does not declare.
	#[error(
		"class `{class}` is not declared. Declare it under `define.classes` in \
		 .source-sentry/source-sentry.yaml"
	)]
	UndeclaredClass {
		/// The class.
		class: ClassName,
	},
	/// An empty owner identifier.
	#[error("owner identifiers cannot be empty")]
	EmptyOwner,
	/// A remote owner whose name cannot name a lockfile.
	#[error(
		"`{owner}` cannot name an owner lockfile; use ASCII letters, digits, \
		 `-`, `_` and `.`, not starting with `.`"
	)]
	InvalidOwnerName {
		/// The owner, as given.
		owner: String,
	},
	/// A reviewer name that cannot be written.
	#[error("the reviewer `{reviewer}` cannot be written: {reason}")]
	InvalidReviewer {
		/// The name, as given.
		reviewer: String,
		/// Why it was rejected.
		reason: &'static str,
	},
	/// A schedule that evaluation would reject.
	#[error("the schedule is invalid: {0}")]
	InvalidSchedule(#[from] ScheduleError),
	/// A `url` for a local document.
	#[error(
		"`{document}` is a local document; only remote documents carry a `url`"
	)]
	UrlNotRemote {
		/// The document.
		document: DocumentId,
	},
	/// A lockfile directory that does not contain the document.
	#[error(
		"`{directory}` does not contain `{document}`, and a lockfile lists only \
		 documents inside its directory"
	)]
	NotAnAncestor {
		/// The document.
		document: RepoPath,
		/// The requested directory.
		directory: RepoPath,
	},
	/// A lockfile directory with a nearer lockfile in between.
	#[error(
		"{nearest} is nearer to `{document}` than `{directory}`, and only the \
		 nearest lockfile may list a document. Use that lockfile, or a \
		 directory below it"
	)]
	NearerLockfile {
		/// The document.
		document: RepoPath,
		/// The requested directory.
		directory: RepoPath,
		/// The nearer lockfile.
		nearest: RepoPath,
	},
	/// A change of authority below a directory that would leave entries
	/// behind in the wrong lockfile: a new lockfile that
	/// would take over another lockfile's entries (splitting), or a nested
	/// lockfile that would keep entries after some of them move to its parent
	/// (merging).
	#[error(
		"the lockfile with authority below `{directory}` changes, and {lockfile} \
		 would keep entries there: {}. Move them in the same `track --from`",
		quoted(.documents)
	)]
	StrandedEntries {
		/// The lockfile that lists them now.
		lockfile: RepoPath,
		/// The directory below which authority changes: the new lockfile's
		/// when splitting, the nested lockfile's when merging.
		directory: RepoPath,
		/// The entries left behind.
		documents: Vec<DocumentId>,
	},
	/// A lockfile that cannot be read or written.
	#[error("{path} cannot be used: {error}. Correct it first")]
	InvalidLockfile {
		/// The lockfile.
		path: RepoPath,
		/// Why.
		error: LockfileError,
	},
	/// Frontmatter that cannot be read or rewritten.
	#[error("the frontmatter of `{document}` cannot be rewritten: {error}")]
	InvalidFrontmatter {
		/// The document.
		document: RepoPath,
		/// Why.
		error: FrontmatterError,
	},
	/// A written file that would not read back as planned. This is a bug in
	/// Source Sentry, never in the repository.
	#[error(
		"{path} would not read back as planned, so nothing was planned; this \
		 is a bug in Source Sentry"
	)]
	RoundTrip {
		/// The file.
		path: RepoPath,
	},
	/// `track --from` naming the wrong current placement.
	#[error("`{document}` is tracked {actual}, not {expected}")]
	WrongOrigin {
		/// The document.
		document: DocumentId,
		/// Where the command said it is.
		expected: Placement,
		/// Where it is.
		actual: Placement,
	},
	/// `track --from` to where the document already is.
	#[error("`{document}` is already tracked {placement}")]
	SamePlacement {
		/// The document.
		document: DocumentId,
		/// Where it is.
		placement: Placement,
	},
	/// `track --from` without documents.
	#[error("name at least one document to move")]
	NothingToMove,
	/// A document named twice in one command.
	#[error("`{document}` is named more than once")]
	RepeatedDocument {
		/// The document.
		document: DocumentId,
	},
	/// A tombstone that a move would carry into a lockfile that already has
	/// one for the same document.
	#[error(
		"`{document}` has tombstones in both {from} and {to}; resolve one of \
		 them in review first"
	)]
	TombstoneConflict {
		/// The document.
		document: DocumentId,
		/// The lockfile the tombstone would leave.
		from: RepoPath,
		/// The lockfile it would join.
		to: RepoPath,
	},
	/// The plan would introduce configuration or integrity errors.
	#[error(
		"the change would introduce these errors, so nothing was planned:\n{}",
		lines(.diagnostics)
	)]
	WouldIntroduce {
		/// The errors that discovery would report after the change.
		diagnostics: Vec<Diagnostic>,
	},
	/// The configs would no longer load after the change.
	#[error(
		"the configs would no longer load after the change, so nothing was \
		 planned:\n{}",
		lines(.errors)
	)]
	InvalidConfigs {
		/// The configuration errors.
		errors: Vec<ConfigError>,
	},
	/// `init` naming a path that is not an unclaimed candidate.
	#[error(
		"`{path}` is not an unclaimed candidate; `init` registers only files \
		 that a root's scan finds and nothing claims"
	)]
	NotACandidate {
		/// The path.
		path: RepoPath,
	},
	/// A lockfile whose schema cannot be read.
	#[error("{path} has no readable schema: {reason}. Correct it first")]
	UnreadableSchema {
		/// The lockfile.
		path: RepoPath,
		/// What is wrong.
		reason: String,
	},
	/// A lockfile schema that this version of Source Sentry cannot migrate.
	#[error(
		"{path} uses lockfile schema {found}, which this version of Source \
		 Sentry does not know; it reads schema {current}. Use a version of \
		 Source Sentry that supports it"
	)]
	UnknownSchema {
		/// The lockfile.
		path: RepoPath,
		/// The schema found.
		found: u64,
		/// The schema this version writes.
		current: u32,
	},
}

/// One difference between the expectations and the snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mismatch {
	/// The snapshot has this state, which no expectation names, such as a
	/// pattern addition the reviewer did not see.
	Unexpected(Expectation),
	/// An expectation the snapshot does not meet.
	Differs {
		/// What was expected.
		expected: Expectation,
		/// What the snapshot holds for the same path, or `None` when the path
		/// is not part of the selection at all.
		actual: Option<Expectation>,
	},
	/// The expectations name this path more than once.
	Repeated(RepoPath),
}

impl fmt::Display for Mismatch {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::Unexpected(actual) => write!(
				formatter,
				"`{actual}` is in the snapshot, but no expectation names it"
			),
			Self::Differs {
				expected,
				actual: Some(actual),
			} => write!(formatter, "expected `{expected}`, found `{actual}`"),
			Self::Differs {
				expected,
				actual: None,
			} => write!(
				formatter,
				"expected `{expected}`, but `{}` is not part of the selection",
				expected.path()
			),
			Self::Repeated(path) => {
				write!(formatter, "`{path}` is named more than once")
			}
		}
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
fn defined_in(config: Option<&RepoPath>) -> String {
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
