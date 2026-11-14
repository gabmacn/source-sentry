//! Batching clock questions, and turning answers into timings with deadlines.
//!
//! Every drift question, including a deleted pattern member's, goes into one
//! [`drift_sources`] call, and every new obligation or completion into one
//! [`starts`] call, so the whole evaluation walks the first-parent history
//! at most twice.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};

use super::history::{Query, starts};
use super::model::{StartedClock, Timing};
use super::scopes::schedule_diagnostic;
use crate::claims::{Claim, Diagnostic, DiagnosticKind, Location};
use crate::evaluate::clock::{
	ClockError, ClockStart, Drift, ObligationStart, SourceQuery, drift_sources,
};
use crate::evaluate::schedule::{Deadline, EffectiveSchedule, TimeZone};
use crate::evaluate::settings::EffectiveSettings;
use crate::model::DocumentId;
use crate::ports::{
	BackendError, BackendErrorKind, History, ObjectId, RepoPath, Revision,
};

/// Where a question's answer will be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Slot {
	/// The answer to a drift question.
	Drift(usize),
	/// The answer to a new obligation or completion question.
	Start(usize),
}

/// The questions gathered so far.
#[derive(Debug, Default)]
pub(super) struct Requests {
	drift: Vec<SourceQuery>,
	/// Each drift question's position, by path and attested oid. The current
	/// state follows from the path, which the snapshot either has or lacks.
	drift_positions: BTreeMap<(RepoPath, ObjectId), usize>,
	starts: Vec<Query>,
}

impl Requests {
	/// Asks when the source at `path`, attested at `attested` and now at
	/// `current`, or absent when `current` is `None`, drifted.
	pub(super) fn drift(
		&mut self,
		path: &RepoPath,
		attested: &ObjectId,
		current: Option<&ObjectId>,
	) -> Slot {
		let key = (path.clone(), attested.clone());
		if let Some(&position) = self.drift_positions.get(&key) {
			return Slot::Drift(position);
		}
		self.drift.push(SourceQuery {
			path: path.clone(),
			attested: attested.clone(),
			current: current.cloned(),
		});
		let position = self.drift.len() - 1;
		self.drift_positions.insert(key, position);
		Slot::Drift(position)
	}

	/// Asks when `query`'s condition started to hold.
	pub(super) fn start(&mut self, query: Query) -> Slot {
		self.starts.push(query);
		Slot::Start(self.starts.len() - 1)
	}

	/// Answers every question. Every slot these requests handed out has an
	/// answer in the result.
	///
	/// # Errors
	///
	/// Fails when the backend fails or a required object is missing.
	pub(super) fn answer<H: History>(
		&self,
		history: &H,
		revision: &Revision,
		at: DateTime<Utc>,
	) -> Result<Answers, BackendError> {
		let answers = Answers {
			drift: drift_sources(history, revision, at, &self.drift)?,
			starts: starts(history, revision, at, &self.starts)?,
		};
		// Both functions promise one answer per question; this keeps a broken
		// promise from surfacing later as a missing answer.
		if answers.drift.len() != self.drift.len()
			|| answers.starts.len() != self.starts.len()
		{
			return Err(BackendError::new(
				BackendErrorKind::Git,
				"the history walk answered a different number of clock questions 				 than were asked",
			));
		}
		Ok(answers)
	}
}

/// Every question's answer.
#[derive(Debug, Default)]
pub(super) struct Answers {
	drift: Vec<Result<Drift, ClockError>>,
	starts: Vec<Result<ObligationStart, ClockError>>,
}

/// One answer, whichever question asked it.
#[derive(Clone, Copy, Debug)]
pub(super) enum Answer<'a> {
	/// The source matches its attestation; no clock runs.
	Fresh,
	/// The clock started here.
	Started(&'a ClockStart),
	/// The obligation or drift exists only in the working tree.
	NotStarted,
	/// The attested oid never appears in the history.
	Unknown,
	/// The clock could not be dated.
	Failed(&'a ClockError),
}

impl Answers {
	/// The answer in `slot`, which must come from the [`Requests`] that
	/// produced these answers.
	pub(super) fn get(&self, slot: Slot) -> Answer<'_> {
		#[expect(
			clippy::expect_used,
			reason = "every slot comes from the requests that `Requests::answer` 			          answered, and it checks that each question has its answer"
		)]
		match slot {
			Slot::Drift(position) => {
				match self
					.drift
					.get(position)
					.expect("a drift answer per question")
				{
					Ok(Drift::Fresh) => Answer::Fresh,
					Ok(Drift::Started(start)) => Answer::Started(start),
					Ok(Drift::NotStarted) => Answer::NotStarted,
					Ok(Drift::TimingUnknown) => Answer::Unknown,
					Err(error) => Answer::Failed(error),
				}
			}
			Slot::Start(position) => {
				match self
					.starts
					.get(position)
					.expect("a start answer per question")
				{
					Ok(ObligationStart::Started(start)) => Answer::Started(start),
					Ok(ObligationStart::NotStarted) => Answer::NotStarted,
					Err(error) => Answer::Failed(error),
				}
			}
		}
	}
}

/// What a clock's diagnostics need to know about the subject it belongs to.
pub(super) struct ClockContext<'a> {
	/// The subject's schedule, or `None` when it is invalid.
	pub(super) schedule: Option<&'a EffectiveSchedule>,
	/// The time zone, or `None` when it is invalid.
	pub(super) zone: Option<&'a TimeZone>,
	/// The subject's settings, whose cadence origin a deadline error names.
	pub(super) settings: &'a EffectiveSettings,
	/// Where a config writes the subject's cadence, such as
	/// `schedule.cadence`.
	pub(super) cadence_field: &'a str,
	/// The declaration the clocks belong to, with its written field lines.
	pub(super) claim: &'a Claim,
	/// The instant of evaluation.
	pub(super) at: DateTime<Utc>,
}

impl ClockContext<'_> {
	/// The timing of an answer, with its deadline. A clock that could not be
	/// dated is reported at `location`, as the clock of `what`.
	pub(super) fn timing(
		&self,
		answer: Answer<'_>,
		location: &Location,
		what: &str,
		diagnostics: &mut Vec<Diagnostic>,
	) -> Timing {
		match answer {
			Answer::Fresh => Timing::Idle,
			Answer::NotStarted => {
				Timing::NotStarted(self.deadline(self.at, diagnostics).map(
					|deadline| Deadline {
						provisional: true,
						..deadline
					},
				))
			}
			Answer::Unknown => Timing::Unknown,
			Answer::Started(start) => Timing::Started(StartedClock {
				start: start.clone(),
				deadline: self
					.deadline(start.timestamp, diagnostics)
					.map(|deadline| self.clamped(start.timestamp, deadline)),
			}),
			Answer::Failed(error) => {
				diagnostics.push(clock_diagnostic(
					error,
					location.clone(),
					&self.claim.document,
					what,
				));
				Timing::Failed
			}
		}
	}

	/// `deadline`, whose clock started at `start`, with the instant it is due
	/// computed as if the clock had started no later than the instant of
	/// evaluation. A clock dated in the future, within the tolerance that
	/// lets it be used, would otherwise make a change due later than it
	/// landed. When it is overdue does not change.
	fn clamped(&self, start: DateTime<Utc>, deadline: Deadline) -> Deadline {
		if start <= self.at {
			return deadline;
		}
		// The deadline from `start` was computable, so an error here can only
		// come from the calendar's edge; it keeps the deadline as it is
		// rather than reporting the schedule twice.
		match self.deadline(self.at, &mut Vec::new()) {
			Some(now) => Deadline {
				due_from: now.due_from.min(deadline.due_from),
				..deadline
			},
			None => deadline,
		}
	}

	/// The deadline of a clock that started at `start`, or
	/// `None` when the schedule or the time zone is invalid, or the deadline
	/// cannot be computed, which is reported.
	fn deadline(
		&self,
		start: DateTime<Utc>,
		diagnostics: &mut Vec<Diagnostic>,
	) -> Option<Deadline> {
		let (schedule, zone) = (self.schedule?, self.zone?);
		match schedule.deadline(start, zone) {
			Ok(deadline) => Some(deadline),
			Err(error) => {
				diagnostics.push(schedule_diagnostic(
					&error,
					&self.settings.cadence.origin,
					self.cadence_field,
					Some(self.claim),
				));
				None
			}
		}
	}
}

/// The diagnostic for a clock that could not be dated: an operational error.
pub(super) fn clock_diagnostic(
	error: &ClockError,
	location: Location,
	document: &DocumentId,
	what: &str,
) -> Diagnostic {
	let (kind, message) = match error {
		ClockError::Shallow(shallow) => (
			DiagnosticKind::ShallowHistory,
			format!(
				"the clock of {what} cannot be dated: {shallow}. Fetch the full \
				 history, for example with `fetch-depth: 0` or `git fetch \
				 --unshallow`; `check` never fetches"
			),
		),
		ClockError::FutureDated(future) => (
			DiagnosticKind::FutureDated,
			format!("the clock of {what} cannot be dated: {future}"),
		),
	};
	Diagnostic::new(kind, location, message).about(document.clone())
}
