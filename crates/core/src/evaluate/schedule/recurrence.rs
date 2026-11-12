//! Cadences: date-only RFC 5545 recurrence rules with a required DTSTART.
//!
//! Rules are expanded by the `rrule` crate. Because cadences are date-only,
//! each rule is expanded from DTSTART at midnight UTC, and only the dates of
//! its occurrences are used; the configured time zone applies afterwards,
//! when dates become instants.

use chrono::{Days, NaiveDate, NaiveTime, TimeZone as _};
use rrule::{Frequency, NWeekday, RRule, RRuleSet, Tz, Unvalidated};

use super::ScheduleError;
use crate::model::Cadence;

/// How far ahead, in days, a search for an occurrence looks: 146 097 days,
/// exactly 400 Gregorian years, the period after which the calendar and its
/// weekdays repeat. See the [module documentation](super#the-search-horizon).
pub const HORIZON_DAYS: u64 = 146_097;

/// A validated cadence: a date-only recurrence rule and its DTSTART, which is
/// the first occurrence.
///
/// Occurrences are dates. The first is DTSTART, and none falls before it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recurrence {
	dtstart: NaiveDate,
	rule: String,
	set: RRuleSet,
}

impl Recurrence {
	/// Validates a declared cadence.
	///
	/// # Errors
	///
	/// - [`ScheduleError::InvalidRule`] when the rule is not the value of an
	///   RRULE property, is not valid RFC 5545, or combines parts the RFC
	///   forbids together.
	/// - [`ScheduleError::TimeOfDay`] for `BYHOUR`, `BYMINUTE`, `BYSECOND`, or a
	///   `FREQ` of `HOURLY`, `MINUTELY` or `SECONDLY`.
	/// - [`ScheduleError::Bounded`] for `COUNT` or `UNTIL`.
	/// - [`ScheduleError::UnsupportedRule`] for a `BYDAY` list that mixes
	///   plain and numbered weekdays, which the evaluator reads wrongly.
	/// - [`ScheduleError::NoOccurrence`] when the rule has no occurrence within
	///   [`HORIZON_DAYS`] of DTSTART, such as a rule matching only 30 February.
	/// - [`ScheduleError::DtstartNotOccurrence`] when DTSTART is not an
	///   occurrence of the rule.
	pub fn new(cadence: &Cadence) -> Result<Self, ScheduleError> {
		let rule = cadence.rrule.as_str();
		let parsed = parse(rule)?;
		check_date_only(rule, &parsed)?;
		check_combinations(rule, &parsed)?;
		check_supported(rule, &parsed)?;
		let start =
			Tz::UTC.from_utc_datetime(&cadence.dtstart.and_time(NaiveTime::MIN));
		let set = parsed
			.build(start)
			.map_err(|error| invalid(rule, error.to_string()))?
			// Bounds every internal search, so a rule without occurrences ends
			// instead of looping to the end of the calendar.
			.limit();
		let recurrence = Self {
			dtstart: cadence.dtstart,
			rule: rule.to_owned(),
			set,
		};
		let first = recurrence.next_on_or_after(cadence.dtstart)?;
		if first != cadence.dtstart {
			return Err(ScheduleError::DtstartNotOccurrence {
				rule: recurrence.rule,
				dtstart: cadence.dtstart,
				first,
			});
		}
		Ok(recurrence)
	}

	/// The first occurrence.
	#[must_use]
	pub fn dtstart(&self) -> NaiveDate {
		self.dtstart
	}

	/// The rule as written.
	#[must_use]
	pub fn rule(&self) -> &str {
		&self.rule
	}

	/// The first occurrence on or after `date`. A date before DTSTART yields
	/// DTSTART.
	///
	/// # Errors
	///
	/// Fails with [`ScheduleError::NoOccurrence`] when no occurrence falls
	/// within [`HORIZON_DAYS`] days on or after `date`.
	pub fn next_on_or_after(
		&self,
		date: NaiveDate,
	) -> Result<NaiveDate, ScheduleError> {
		let limit = date
			.checked_add_days(Days::new(HORIZON_DAYS))
			.unwrap_or(NaiveDate::MAX);
		match self.dates().find(|&occurrence| occurrence >= date) {
			Some(occurrence) if occurrence <= limit => Ok(occurrence),
			_ => Err(ScheduleError::NoOccurrence {
				rule: self.rule.clone(),
				from: date,
			}),
		}
	}

	/// Every occurrence from `first` to `last`, both included, in order. The
	/// list is empty when `last` precedes `first`.
	#[must_use]
	pub fn occurrences(
		&self,
		first: NaiveDate,
		last: NaiveDate,
	) -> Vec<NaiveDate> {
		self
			.dates()
			.skip_while(|&occurrence| occurrence < first)
			.take_while(|&occurrence| occurrence <= last)
			.collect()
	}

	/// Every occurrence from DTSTART on, in order.
	fn dates(&self) -> impl Iterator<Item = NaiveDate> + '_ {
		(&self.set)
			.into_iter()
			.map(|occurrence| occurrence.date_naive())
	}
}

/// Parses the value of an RRULE property.
fn parse(rule: &str) -> Result<RRule<Unvalidated>, ScheduleError> {
	// The parser also accepts whole content lines, including `EXRULE:` ones,
	// which it reads as RRULEs. The field holds a value only.
	if rule.contains([':', '\n', '\r']) {
		return Err(invalid(
			rule,
			"write only the rule's value, such as `FREQ=MONTHLY;INTERVAL=2`, \
			 without `RRULE:` or other properties",
		));
	}
	check_parts(rule)?;
	rule
		.parse()
		.map_err(|error: rrule::RRuleError| invalid(rule, error.to_string()))
}

/// Checks written parts before the dependency discards or normalises them.
fn check_parts(rule: &str) -> Result<(), ScheduleError> {
	for component in rule.split(';') {
		let Some((name, value)) = component.split_once('=') else {
			return Err(invalid(rule, "each rule part must be `NAME=value`"));
		};
		if !name.is_ascii() {
			return Err(invalid(rule, "rule part names must use ASCII letters"));
		}
		let name = name.to_ascii_uppercase();
		let time_of_day = match name.as_str() {
			"BYHOUR" => Some("BYHOUR"),
			"BYMINUTE" => Some("BYMINUTE"),
			"BYSECOND" => Some("BYSECOND"),
			_ => None,
		};
		if let Some(part) = time_of_day {
			return Err(ScheduleError::TimeOfDay {
				rule: rule.to_owned(),
				part,
			});
		}
		let bound = match name.as_str() {
			"COUNT" => Some("COUNT"),
			"UNTIL" => Some("UNTIL"),
			_ => None,
		};
		if let Some(part) = bound {
			return Err(ScheduleError::Bounded {
				rule: rule.to_owned(),
				part,
			});
		}
		if name.is_empty() || value.split(',').any(str::is_empty) {
			return Err(invalid(
				rule,
				"rule part names and values must not be empty",
			));
		}
		match name.as_str() {
			"BYDAY" | "BYWEEKDAY" => check_weekdays(rule, value)?,
			"BYMONTHDAY"
				if value.split(',').any(|day| day.parse::<i16>() == Ok(0)) =>
			{
				return Err(invalid(rule, "BYMONTHDAY values must not be zero"));
			}
			_ => {}
		}
	}
	Ok(())
}

/// Checks ASCII weekday names and optional signed, nonzero ordinals from
/// 1 to 53 before the dependency slices or normalises the tokens.
fn check_weekdays(rule: &str, value: &str) -> Result<(), ScheduleError> {
	for token in value.split(',') {
		let bad_token = || {
			invalid(
				rule,
				format!(
					"`{token}` is not a valid BYDAY value; use a weekday such as \
					 `MO`, optionally preceded by a nonzero ordinal from -53 to 53"
				),
			)
		};
		let (ordinal, weekday) = token
			.split_at_checked(token.len().saturating_sub(2))
			.ok_or_else(bad_token)?;
		if !matches!(
			weekday.to_ascii_uppercase().as_str(),
			"MO" | "TU" | "WE" | "TH" | "FR" | "SA" | "SU"
		) {
			return Err(bad_token());
		}
		if !ordinal.is_empty() {
			let digits = ordinal.strip_prefix(['+', '-']).unwrap_or(ordinal);
			if !(1..=2).contains(&digits.len())
				|| !digits.bytes().all(|byte| byte.is_ascii_digit())
				|| !digits
					.parse::<u8>()
					.is_ok_and(|number| (1..=53).contains(&number))
			{
				return Err(bad_token());
			}
		}
	}
	Ok(())
}

/// Rejects time-of-day frequencies after the written properties are checked.
fn check_date_only(
	rule: &str,
	parsed: &RRule<Unvalidated>,
) -> Result<(), ScheduleError> {
	let time_of_day = match parsed.get_freq() {
		Frequency::Hourly => Some("FREQ=HOURLY"),
		Frequency::Minutely => Some("FREQ=MINUTELY"),
		Frequency::Secondly => Some("FREQ=SECONDLY"),
		Frequency::Yearly
		| Frequency::Monthly
		| Frequency::Weekly
		| Frequency::Daily => None,
	};
	if let Some(part) = time_of_day {
		return Err(ScheduleError::TimeOfDay {
			rule: rule.to_owned(),
			part,
		});
	}
	Ok(())
}

/// Enforces RFC 5545's restrictions that the evaluator does not, because it
/// would expand the rule into dates the RFC does not define.
fn check_combinations(
	rule: &str,
	parsed: &RRule<Unvalidated>,
) -> Result<(), ScheduleError> {
	let freq = parsed.get_freq();
	let numbered = parsed
		.get_by_weekday()
		.iter()
		.any(|weekday| matches!(weekday, NWeekday::Nth(..)));
	let reason = if parsed.get_interval() == 0 {
		// The evaluator would yield no dates at all.
		Some("INTERVAL must be a positive integer")
	} else if numbered && !matches!(freq, Frequency::Monthly | Frequency::Yearly)
	{
		// The evaluator would yield every day.
		Some("numbered BYDAY values, such as `1MO`, need FREQ=MONTHLY or YEARLY")
	} else if numbered && !parsed.get_by_week_no().is_empty() {
		Some("numbered BYDAY values, such as `1MO`, cannot be used with BYWEEKNO")
	} else if freq == Frequency::Weekly && !parsed.get_by_month_day().is_empty() {
		// The evaluator rejects positive values, but accepts negative ones.
		Some("BYMONTHDAY cannot be used with FREQ=WEEKLY")
	} else if !parsed.get_by_set_pos().is_empty() && !has_other_by_part(parsed) {
		// The evaluator would read the position against DTSTART's own day.
		Some("BYSETPOS needs another BY rule part to select from")
	} else {
		None
	};
	match reason {
		Some(reason) => Err(invalid(rule, reason)),
		None => Ok(()),
	}
}

/// Whether a rule part other than `BYSETPOS` selects dates. Time-of-day parts
/// are already rejected.
fn has_other_by_part(parsed: &RRule<Unvalidated>) -> bool {
	!(parsed.get_by_month().is_empty()
		&& parsed.get_by_month_day().is_empty()
		&& parsed.get_by_year_day().is_empty()
		&& parsed.get_by_week_no().is_empty()
		&& parsed.get_by_weekday().is_empty())
}

/// Rejects valid rules that the evaluator would expand wrongly.
fn check_supported(
	rule: &str,
	parsed: &RRule<Unvalidated>,
) -> Result<(), ScheduleError> {
	let weekdays = parsed.get_by_weekday();
	let plain = weekdays
		.iter()
		.any(|weekday| matches!(weekday, NWeekday::Every(_)));
	let numbered = weekdays
		.iter()
		.any(|weekday| matches!(weekday, NWeekday::Nth(..)));
	// RFC 5545 reads a BYDAY list as a union, but the evaluator intersects
	// plain and numbered weekdays: `BYDAY=FR,1FR` would yield first Fridays
	// only, instead of every Friday.
	if plain && numbered {
		return Err(ScheduleError::UnsupportedRule {
			rule: rule.to_owned(),
			reason: "a BYDAY list cannot mix plain weekdays, such as `MO`, with \
			         numbered ones, such as `1FR`"
				.to_owned(),
		});
	}
	Ok(())
}

/// An [`ScheduleError::InvalidRule`] for `rule`.
fn invalid(rule: &str, reason: impl Into<String>) -> ScheduleError {
	ScheduleError::InvalidRule {
		rule: rule.to_owned(),
		reason: reason.into(),
	}
}
