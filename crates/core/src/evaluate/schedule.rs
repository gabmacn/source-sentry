//! Schedules: grace periods, RRULE cadences, time zones and deadlines.
//!
//! # Cadences
//!
//! A cadence is an RFC 5545 recurrence rule with a required, date-only
//! DTSTART. [`Recurrence::new`] validates it:
//!
//! - The rule is the value of an RRULE property alone, such as
//!   `FREQ=MONTHLY;INTERVAL=2`, without `RRULE:` or any other property.
//! - It has no time-of-day part: no `BYHOUR`, `BYMINUTE` or `BYSECOND`, and no
//!   `FREQ` of `HOURLY`, `MINUTELY` or `SECONDLY`.
//! - It never ends: no `COUNT` and no `UNTIL`.
//! - It follows the RFC's restrictions on combining rule parts, including
//!   those the recurrence evaluator would otherwise accept.
//! - DTSTART is its first occurrence, because DTSTART is defined as
//!   the first due date.
//! - It has an occurrence within [`HORIZON_DAYS`] of DTSTART.
//!
//! # The search horizon
//!
//! Every search for a next occurrence looks [`HORIZON_DAYS`] days ahead:
//! 146 097 days, exactly 400 Gregorian years. The Gregorian calendar repeats
//! itself, weekdays included, every 400 years, so a rule with `INTERVAL=1`
//! that has no occurrence within that span has none at all. This covers every
//! leap-day rule, such as `FREQ=YEARLY;BYMONTH=2;BYMONTHDAY=29`, which skips
//! 2100, 2200 and 2300. A rule with a larger interval can, in theory, leave a
//! gap of more than 400 years between occurrences; a search that crosses such
//! a gap fails with [`ScheduleError::NoOccurrence`]. Occurrences end in the
//! year 10 000, where the evaluator stops.
//!
//! # Deadlines
//!
//! [`EffectiveSchedule::deadline`] computes when a document becomes overdue,
//! given the instant its drift started. Every date is a local date
//! in the configured [`TimeZone`]. Let `start_date` be the local date of the
//! start instant, and `g` the grace period in days:
//!
//! - **No cadence, `g = 0`.** The document is due at once: it is overdue from
//!   the start instant itself.
//! - **No cadence, `g > 0`.** It is due by the end of the local day
//!   `start_date + g`, so it is overdue from the first instant of the local day
//!   after that.
//! - **With a cadence.** The due date `d` is the first occurrence with
//!   `d >= start_date + g`, and the document is overdue from the first instant
//!   of the local day after `d`. With `g = 0`, `d` may be `start_date` itself.
//!
//! The first instant of a local day is its midnight. When daylight saving
//! time skips that midnight, it is the first instant after the skipped hour,
//! the moment the clocks jump; when midnight occurs twice, it is the earlier
//! of the two.

mod recurrence;
mod zone;

#[cfg(test)]
mod tests;

use chrono::{DateTime, Days, NaiveDate, Utc};

pub use self::{
	recurrence::{HORIZON_DAYS, Recurrence},
	zone::TimeZone,
};
use crate::model::Schedule;

/// Why a schedule, a cadence or a time zone was rejected, or why a date could
/// not be computed.
///
/// Every variant except [`ScheduleError::OutOfRange`] describes a problem in
/// the configured schedule itself. [`ScheduleError::OutOfRange`] means that a
/// computation left the calendar the evaluator supports.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ScheduleError {
	/// The name is not an IANA time zone name in the bundled database. Names
	/// are matched exactly, including case, and fixed offsets such as `+01:00`
	/// are not names.
	#[error(
		"`{0}` is not an IANA time zone name, such as `Europe/Madrid`; names are \
		 case-sensitive, and fixed offsets such as `+01:00` are not accepted"
	)]
	UnknownTimeZone(String),
	/// The rule is not a valid RFC 5545 recurrence rule value.
	#[error("`{rule}` is not a valid recurrence rule: {reason}")]
	InvalidRule {
		/// The rule as written.
		rule: String,
		/// What is wrong with it.
		reason: String,
	},
	/// The rule has a time-of-day part, but cadences are date-only.
	#[error("`{rule}` uses {part}, but cadences are date-only")]
	TimeOfDay {
		/// The rule as written.
		rule: String,
		/// The offending part, such as `BYHOUR` or `FREQ=HOURLY`.
		part: &'static str,
	},
	/// The rule ends, but a cadence never does.
	#[error("`{rule}` uses {part}, but a cadence never ends")]
	Bounded {
		/// The rule as written.
		rule: String,
		/// `COUNT` or `UNTIL`.
		part: &'static str,
	},
	/// The rule is valid RFC 5545, but the recurrence evaluator would compute
	/// different dates from those the RFC defines, so it is rejected rather
	/// than evaluated wrongly.
	#[error("`{rule}` is not supported: {reason}")]
	UnsupportedRule {
		/// The rule as written.
		rule: String,
		/// What is not supported, and how to express it instead.
		reason: String,
	},
	/// The rule has no occurrence within [`HORIZON_DAYS`] of `from`.
	#[error(
		"`{rule}` has no occurrence within 400 years (146 097 days) from {from}"
	)]
	NoOccurrence {
		/// The rule as written.
		rule: String,
		/// The date the search started from.
		from: NaiveDate,
	},
	/// DTSTART is not an occurrence of the rule, so the first due date would be
	/// ambiguous.
	#[error(
		"dtstart {dtstart} is not an occurrence of `{rule}`; set dtstart to the \
		 first due date, such as {first}"
	)]
	DtstartNotOccurrence {
		/// The rule as written.
		rule: String,
		/// The declared DTSTART.
		dtstart: NaiveDate,
		/// The rule's first occurrence after DTSTART.
		first: NaiveDate,
	},
	/// A date or instant fell outside the supported calendar, the years -9999
	/// to 9999.
	#[error("a date or instant falls outside the supported calendar")]
	OutOfRange,
}

/// The schedule that applies to a document once inheritance is resolved.
///
/// The default is a grace period of zero days and no cadence: a document is
/// due as soon as a source changes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EffectiveSchedule {
	/// Whole days a document may lag behind a changed source.
	pub grace: u32,
	/// The validated cadence that due dates fall on, if any.
	pub cadence: Option<Recurrence>,
}

impl EffectiveSchedule {
	/// The effective form of a resolved schedule. A grace period that nothing
	/// set is zero days.
	///
	/// # Errors
	///
	/// Fails when the cadence is invalid; see [`Recurrence::new`].
	pub fn new(schedule: &Schedule) -> Result<Self, ScheduleError> {
		Ok(Self {
			grace: schedule.grace.unwrap_or(0),
			cadence: schedule.cadence.as_set().map(Recurrence::new).transpose()?,
		})
	}

	/// When a document whose drift started at `start` becomes overdue, with
	/// dates read in `zone`. The rules are described in the
	/// [module documentation](self#deadlines).
	///
	/// # Errors
	///
	/// - [`ScheduleError::NoOccurrence`] when the cadence has no occurrence
	///   within [`HORIZON_DAYS`] of the earliest possible due date.
	/// - [`ScheduleError::OutOfRange`] when a date leaves the supported
	///   calendar, for example because of an enormous grace period.
	pub fn deadline(
		&self,
		start: DateTime<Utc>,
		zone: &TimeZone,
	) -> Result<Deadline, ScheduleError> {
		if self.grace == 0 && self.cadence.is_none() {
			return Ok(Deadline {
				due: None,
				overdue_from: start,
			});
		}
		let earliest = add_days(zone.local_date(start)?, self.grace)?;
		let due = match &self.cadence {
			Some(cadence) => cadence.next_on_or_after(earliest)?,
			None => earliest,
		};
		Ok(Deadline {
			due: Some(due),
			overdue_from: zone.start_of_day(add_days(due, 1)?)?,
		})
	}
}

/// When a document becomes overdue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Deadline {
	/// The last local day on which the document is on time, or `None` when it
	/// is due at once.
	pub due: Option<NaiveDate>,
	/// The first instant at which the document is overdue: the first instant of
	/// the local day after `due`, or the drift start when it is due at once.
	pub overdue_from: DateTime<Utc>,
}

impl Deadline {
	/// Whether the document is overdue at `at`.
	#[must_use]
	pub fn is_overdue(&self, at: DateTime<Utc>) -> bool {
		at >= self.overdue_from
	}
}

/// `date` plus `days` whole days.
///
/// # Errors
///
/// Fails with [`ScheduleError::OutOfRange`] when the result is not a
/// representable date.
pub(super) fn add_days(
	date: NaiveDate,
	days: u32,
) -> Result<NaiveDate, ScheduleError> {
	date
		.checked_add_days(Days::new(u64::from(days)))
		.ok_or(ScheduleError::OutOfRange)
}
