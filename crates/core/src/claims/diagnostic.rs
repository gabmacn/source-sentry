//! Diagnostics: what discovery and evaluation found wrong, how severe it is,
//! and where.
//!
//! Diagnostics are data for the report module to render. Each carries a
//! stable [`DiagnosticKind`], which fixes its [`Severity`], a message that
//! names what to fix, the [`Location`] to fix it at, any related locations,
//! and the document it concerns.
//!
//! Discovery ([`discover`](super::discover)) reports problems with claims,
//! sources, series membership and paths. Evaluation
//! ([`check`](crate::evaluate::check)) adds its own kinds to the same type:
//! the context and the time zone, schedules, canonicalization, clocks, seals
//! and the heads of `living` series.

use std::fmt;

use crate::evaluate::schedule::ScheduleError;
use crate::model::{DocumentId, RepoPath};
use crate::policy::ErrorKind;

/// How severe a diagnostic is.
///
/// Ordered from least to most severe.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
	/// Probably a mistake, but evaluation can proceed: a pattern with no
	/// matches, or paths that differ only in case. A
	/// warning is neither a fact nor an error, so no context maps it and it
	/// never sets the exit code on its own.
	Warning,
	/// A configuration error, which exits 3 and which no context can
	/// downgrade.
	Configuration,
	/// An integrity error: what Source Sentry protects is broken, such as a
	/// double claim or a missing sealed record. It exits 3, and no context can
	/// downgrade it.
	Integrity,
	/// An operational error: the evaluation could not complete, for example
	/// because a file cannot be canonicalized or the history is shallow. It
	/// exits 4, and no context can downgrade it.
	Operational,
}

impl Severity {
	/// The policy module's kind of error for a fatal severity, or `None` for
	/// a warning.
	#[must_use]
	pub fn error_kind(self) -> Option<ErrorKind> {
		match self {
			Self::Warning => None,
			Self::Configuration => Some(ErrorKind::Configuration),
			Self::Integrity => Some(ErrorKind::Integrity),
			Self::Operational => Some(ErrorKind::Operational),
		}
	}

	/// Whether the severity is an error: a configuration or integrity error,
	/// which exits 3, or an operational error, which exits 4.
	#[must_use]
	pub fn is_fatal(self) -> bool {
		self.error_kind().is_some()
	}

	/// The name used in reports, in kebab-case.
	#[must_use]
	pub fn name(self) -> &'static str {
		match self {
			Self::Warning => "warning",
			Self::Configuration => "configuration-error",
			Self::Integrity => "integrity-error",
			Self::Operational => "operational-error",
		}
	}
}

impl fmt::Display for Severity {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		formatter.write_str(self.name())
	}
}

/// What a diagnostic is about. Each kind has a stable [`code`](Self::code)
/// and a fixed [`severity`](Self::severity).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DiagnosticKind {
	/// A lockfile that cannot be parsed, so none of its documents or
	/// tombstones are known.
	InvalidLockfile,
	/// An on-file declaration that cannot be read.
	InvalidDeclaration,
	/// A document claimed more than once: on-file and in a lockfile, in two
	/// lockfiles, or one remote identity in two owner lockfiles. An
	/// integrity error.
	DoubleClaim,
	/// A remote identity that one owner lockfile lists or retires while
	/// another retires it, although an identity may appear in only one owner
	/// lockfile.
	RetiredInAnotherLockfile,
	/// A local lockfile that lists or retires a document for which a nested
	/// lockfile is nearer, although only the nearest ancestor lockfile may
	/// list a document.
	OutsideAuthority,
	/// A document that a local lockfile lists but the snapshot lacks.
	MissingDocument,
	/// A sealed record that a local lockfile lists but the snapshot lacks,
	/// without a tombstone. An integrity error.
	MissingRecord,
	/// A declaration that assigns a class the repository config does not
	/// declare.
	UndeclaredClass,
	/// A source or member path that cannot be resolved.
	InvalidSourcePath,
	/// An explicitly named source of a live obligation that the snapshot
	/// lacks.
	MissingSource,
	/// A pattern source that cannot be compiled or resolved.
	InvalidPattern,
	/// A path or pattern source that resolves to the document itself.
	SelfDependency,
	/// A pattern source that matches no tracked path. A warning.
	EmptyPattern,
	/// A source that is an on-file document, which must use lockfile
	/// tracking instead.
	OnFileSource,
	/// Documents that are sources of each other in a cycle.
	DependencyCycle,
	/// A file that matches more than one series.
	SeriesOverlap,
	/// Two members of a series with the same parsed version.
	DuplicateVersion,
	/// A series member whose version does not parse as its kind.
	InvalidVersion,
	/// Tracked paths in one directory whose names differ only in case. A
	/// warning.
	CaseCollision,
	/// A config cannot be loaded, so nothing was evaluated. Reported by
	/// [`crate::evaluate::unloadable`], except for invalid schedules, which keep
	/// their own diagnostic kind.
	InvalidConfig,
	/// The requested context is not declared, so no outcome can be decided.
	/// Reported by evaluation.
	UnknownContext,
	/// A declared context has invalid rules. Reported by
	/// evaluation, which validates every declared context.
	InvalidContext,
	/// The repository config pins a default-policy version that this build
	/// does not ship. Reported by evaluation.
	UnsupportedDefaultsVersion,
	/// The built-in default context shipped with this build cannot be read,
	/// which indicates a defective build. Reported by evaluation.
	BrokenDefaults,
	/// The configured time zone is not an IANA name.
	/// Reported by evaluation.
	InvalidTimeZone,
	/// A cadence that is invalid, reported when configs or declarations load,
	/// or that has no due date where evaluation needs one.
	InvalidSchedule,
	/// A source that is a submodule, which Source Sentry does not support.
	/// Reported by evaluation, at the declaration that
	/// names it.
	Submodule,
	/// A `living` series whose newest entry is sealed. Reported by
	/// evaluation.
	SealedHead,
	/// A `living` series whose newest entry is missing from the snapshot or
	/// only retired. Reported by evaluation.
	MissingHead,
	/// A sealed record whose canonical oid differs from its seal. An
	/// integrity error, reported by evaluation.
	BrokenSeal,
	/// A file to which an external clean or process filter applies, so no oid
	/// was guessed. An operational error, reported by evaluation.
	UnsupportedFilter,
	/// A file whose Git LFS pointer uses extensions. An
	/// operational error, reported by evaluation.
	UnsupportedLfsExtension,
	/// A file that the index holds with CRLF line endings although its
	/// attributes ask git to normalize it. An operational error,
	/// reported by evaluation.
	CrlfInIndex,
	/// A clock whose search reached a shallow clone's boundary before
	/// concluding. An operational error, reported by evaluation.
	ShallowHistory,
	/// A clock whose timestamp lies more than the tolerance after the
	/// evaluation instant. An operational error, reported by
	/// evaluation.
	FutureDated,
	/// A deadline or cadence window that falls outside the calendar the
	/// evaluator supports. An operational error, reported by evaluation.
	DateOutOfRange,
}

impl DiagnosticKind {
	/// The stable code, in kebab-case, for reports and JSON output.
	#[must_use]
	pub fn code(self) -> &'static str {
		match self {
			Self::InvalidLockfile => "invalid-lockfile",
			Self::InvalidDeclaration => "invalid-declaration",
			Self::DoubleClaim => "double-claim",
			Self::RetiredInAnotherLockfile => "retired-in-another-lockfile",
			Self::OutsideAuthority => "outside-authority",
			Self::MissingDocument => "missing-document",
			Self::MissingRecord => "missing-record",
			Self::UndeclaredClass => "undeclared-class",
			Self::InvalidSourcePath => "invalid-source-path",
			Self::MissingSource => "missing-source",
			Self::InvalidPattern => "invalid-pattern",
			Self::SelfDependency => "self-dependency",
			Self::EmptyPattern => "empty-pattern",
			Self::OnFileSource => "on-file-source",
			Self::DependencyCycle => "dependency-cycle",
			Self::SeriesOverlap => "series-overlap",
			Self::DuplicateVersion => "duplicate-version",
			Self::InvalidVersion => "invalid-version",
			Self::CaseCollision => "case-collision",
			Self::InvalidConfig => "invalid-config",
			Self::UnknownContext => "unknown-context",
			Self::InvalidContext => "invalid-context",
			Self::UnsupportedDefaultsVersion => "unsupported-defaults-version",
			Self::BrokenDefaults => "broken-defaults",
			Self::InvalidTimeZone => "invalid-time-zone",
			Self::InvalidSchedule => "invalid-schedule",
			Self::Submodule => "submodule",
			Self::SealedHead => "sealed-head",
			Self::MissingHead => "missing-head",
			Self::BrokenSeal => "broken-seal",
			Self::UnsupportedFilter => "unsupported-filter",
			Self::UnsupportedLfsExtension => "unsupported-lfs-extension",
			Self::CrlfInIndex => "crlf-in-index",
			Self::ShallowHistory => "shallow-history",
			Self::FutureDated => "future-dated",
			Self::DateOutOfRange => "date-out-of-range",
		}
	}

	/// The severity every diagnostic of this kind has.
	#[must_use]
	pub fn severity(self) -> Severity {
		match self {
			Self::EmptyPattern | Self::CaseCollision => Severity::Warning,
			Self::DoubleClaim | Self::MissingRecord | Self::BrokenSeal => {
				Severity::Integrity
			}
			Self::UnsupportedFilter
			| Self::UnsupportedLfsExtension
			| Self::CrlfInIndex
			| Self::ShallowHistory
			| Self::FutureDated
			| Self::DateOutOfRange => Severity::Operational,
			Self::InvalidLockfile
			| Self::InvalidDeclaration
			| Self::RetiredInAnotherLockfile
			| Self::OutsideAuthority
			| Self::MissingDocument
			| Self::UndeclaredClass
			| Self::InvalidSourcePath
			| Self::MissingSource
			| Self::InvalidPattern
			| Self::SelfDependency
			| Self::OnFileSource
			| Self::DependencyCycle
			| Self::SeriesOverlap
			| Self::DuplicateVersion
			| Self::InvalidVersion
			| Self::InvalidConfig
			| Self::UnknownContext
			| Self::InvalidContext
			| Self::UnsupportedDefaultsVersion
			| Self::BrokenDefaults
			| Self::InvalidTimeZone
			| Self::InvalidSchedule
			| Self::Submodule
			| Self::SealedHead
			| Self::MissingHead => Severity::Configuration,
		}
	}
}

impl fmt::Display for DiagnosticKind {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		formatter.write_str(self.code())
	}
}

/// Where a diagnostic points: a file, and within it a line, a column or a
/// field when they are known.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Location {
	/// The file to fix: a lockfile, a document or a config. For a document
	/// that a lockfile lists but the snapshot lacks, the path it should have.
	pub file: RepoPath,
	/// The line, counted from 1.
	pub line: Option<u64>,
	/// The column, in characters, counted from 1.
	pub column: Option<u64>,
	/// The field, such as `documents["a.md"].sources[0].path` in a lockfile
	/// or `source-sentry.classes[1]` in frontmatter.
	pub field: Option<String>,
}

impl Location {
	/// The whole of `file`.
	#[must_use]
	pub fn file(file: RepoPath) -> Self {
		Self {
			file,
			line: None,
			column: None,
			field: None,
		}
	}

	/// The field `field` of `file`.
	#[must_use]
	pub fn field(file: RepoPath, field: impl Into<String>) -> Self {
		Self {
			field: Some(field.into()),
			..Self::file(file)
		}
	}

	/// The same location, at `line` and `column` where they are known.
	#[must_use]
	pub fn at(mut self, line: Option<u64>, column: Option<u64>) -> Self {
		self.line = line;
		self.column = column;
		self
	}
}

impl fmt::Display for Location {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(formatter, "{}", self.file)?;
		if let Some(line) = self.line {
			write!(formatter, ":{line}")?;
			if let Some(column) = self.column {
				write!(formatter, ":{column}")?;
			}
		}
		if let Some(field) = &self.field {
			write!(formatter, " `{field}`")?;
		}
		Ok(())
	}
}

/// One problem that discovery or evaluation found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
	/// What the problem is, which fixes its severity.
	pub kind: DiagnosticKind,
	/// What is wrong and what to do about it, in a sentence or two.
	pub message: String,
	/// Where to fix it.
	pub location: Location,
	/// Other places involved, such as the other declarations of a double
	/// claim, sorted.
	pub related: Vec<Location>,
	/// The document the problem concerns, when it concerns one.
	pub document: Option<DocumentId>,
}

impl Diagnostic {
	/// A schedule error at its written cadence or resolved setting. Calendar
	/// overflow remains operational; other schedule errors are configuration
	/// errors.
	pub(crate) fn schedule(error: &ScheduleError, location: Location) -> Self {
		let (kind, message) = match error {
			ScheduleError::OutOfRange => (
				DiagnosticKind::DateOutOfRange,
				format!(
					"{error}, so a deadline or cadence window could not be computed and \
					 the evaluation is incomplete. Check the grace period and the \
					 cadence's dates"
				),
			),
			ScheduleError::UnknownTimeZone(_)
			| ScheduleError::InvalidRule { .. }
			| ScheduleError::TimeOfDay { .. }
			| ScheduleError::Bounded { .. }
			| ScheduleError::UnsupportedRule { .. }
			| ScheduleError::NoOccurrence { .. }
			| ScheduleError::DtstartNotOccurrence { .. } => (
				DiagnosticKind::InvalidSchedule,
				format!(
					"{error}. Until the cadence is corrected, the documents and series \
					 that use it have no deadline"
				),
			),
		};
		Self::new(kind, location, message)
	}

	/// A diagnostic of `kind` at `location`, about no document in particular
	/// and with no related locations.
	#[must_use]
	pub fn new(
		kind: DiagnosticKind,
		location: Location,
		message: impl Into<String>,
	) -> Self {
		Self {
			kind,
			message: message.into(),
			location,
			related: Vec::new(),
			document: None,
		}
	}

	/// The same diagnostic, about `document`.
	#[must_use]
	pub fn about(mut self, document: DocumentId) -> Self {
		self.document = Some(document);
		self
	}

	/// The same diagnostic, with `related` locations, sorted and without
	/// repeats.
	#[must_use]
	pub fn with_related(mut self, mut related: Vec<Location>) -> Self {
		related.sort();
		related.dedup();
		self.related = related;
		self
	}

	/// The severity, which the kind fixes.
	#[must_use]
	pub fn severity(&self) -> Severity {
		self.kind.severity()
	}

	/// The order diagnostics are reported in: by file, line, column and
	/// field, then by kind and message.
	pub(crate) fn sort_key(
		&self,
	) -> (&Location, DiagnosticKind, &str, &Option<DocumentId>) {
		(&self.location, self.kind, &self.message, &self.document)
	}
}

impl fmt::Display for Diagnostic {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(
			formatter,
			"{} [{}] {}: {}",
			self.severity(),
			self.kind,
			self.location,
			self.message
		)
	}
}
