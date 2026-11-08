//! Evaluation: drift, clocks, schedules, seals and cadence, producing facts,
//! judgements and exit codes.
//!
//! [`check`] is the function behind the `check` command. It takes a
//! snapshot, its history, its configs and its inventory, an evaluation
//! instant and a context, and returns an [`Evaluation`]: every subject with
//! its facts, judgement and evidence, every diagnostic, and the summary that
//! gives the exit code under either threshold.
//!
//! Its building blocks are public too:
//!
//! - [`clock`] dates drift and new obligations from first-parent history;
//! - [`schedule`] turns a grace period and a cadence into deadlines;
//! - [`cadence`] evaluates a series' cadence windows;
//! - [`settings`] combines a document's declaration with its defaults.

pub mod cadence;
pub mod clock;
mod run;
pub mod schedule;
pub mod settings;

pub use run::{
	AppliedContext, AppliedRule, CompletionState, DocumentState, DocumentSubject,
	Evaluation, Gates, LiveDocument, MemberCompletion, RecordCheck, RecordState,
	RuleOrigin, SeriesCadence, SeriesKey, SeriesSubject, SourceReport,
	SourceState, StartedClock, Subject, SubjectKind, Timing, check,
};
