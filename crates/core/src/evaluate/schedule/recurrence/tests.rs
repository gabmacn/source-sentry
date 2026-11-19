//! Tests that searches over the shared expansion find what a new expansion
//! from DTSTART finds.

use jiff::{Span, civil::Date};

use super::{HORIZON_DAYS, Recurrence, from_naive};
use crate::evaluate::schedule::{ScheduleError, add_days};
use crate::model::Cadence;

/// Rules of every frequency, with intervals, month ends, numbered weekdays,
/// set positions, week numbers, year days and leap days.
const RULES: [&str; 13] = [
	"FREQ=DAILY",
	"FREQ=DAILY;INTERVAL=3",
	"FREQ=WEEKLY;BYDAY=MO,WE,FR",
	"FREQ=WEEKLY;INTERVAL=2;BYDAY=TU",
	"FREQ=MONTHLY",
	"FREQ=MONTHLY;BYMONTHDAY=31",
	"FREQ=MONTHLY;BYMONTHDAY=-1",
	"FREQ=MONTHLY;INTERVAL=5;BYDAY=-1FR",
	"FREQ=MONTHLY;BYDAY=MO,TU,WE,TH,FR;BYSETPOS=-1",
	"FREQ=YEARLY;BYMONTH=2;BYMONTHDAY=29",
	"FREQ=YEARLY;BYWEEKNO=20;BYDAY=MO",
	"FREQ=YEARLY;INTERVAL=7;BYYEARDAY=100,-1",
	"FREQ=YEARLY;BYMONTH=1,7;BYDAY=1SU",
];

/// Start dates from the first year of the calendar to its last.
const STARTS: [&str; 6] = [
	"0001-01-01",
	"1582-10-04",
	"1900-01-01",
	"2000-02-29",
	"2026-11-15",
	"9998-06-01",
];

/// `rule` from its first occurrence on or after `start`, or `None` when it
/// has none there or that occurrence falls after the year 9999.
fn cadence(rule: &str, start: &str) -> Option<Recurrence> {
	let cadence = |dtstart| Cadence {
		dtstart,
		rrule: rule.to_owned(),
	};
	match Recurrence::new(&cadence(start.parse().unwrap())) {
		Ok(recurrence) => Some(recurrence),
		Err(ScheduleError::DtstartNotOccurrence { first, .. }) => {
			Some(Recurrence::new(&cadence(first)).unwrap())
		}
		Err(ScheduleError::NoOccurrence { .. } | ScheduleError::OutOfRange) => None,
		Err(error) => panic!("{rule} from {start}: {error}"),
	}
}

/// Every occurrence from DTSTART on, from a new expansion, each `None` when
/// it falls after 31 December 9999.
fn fresh(recurrence: &Recurrence) -> impl Iterator<Item = Option<Date>> + '_ {
	(&recurrence.set)
		.into_iter()
		.map(|occurrence| from_naive(occurrence.date_naive()))
}

/// [`Recurrence::next_on_or_after`], from a new expansion.
fn next_by_new_expansion(
	recurrence: &Recurrence,
	date: Date,
) -> Result<Date, ScheduleError> {
	let limit = add_days(date, HORIZON_DAYS);
	let found = fresh(recurrence)
		.map_while(|occurrence| occurrence)
		.find(|&occurrence| occurrence >= date);
	match (found, limit) {
		(Some(occurrence), Ok(limit)) if occurrence <= limit => Ok(occurrence),
		(Some(occurrence), Err(_)) => Ok(occurrence),
		(None, Err(_)) => Err(ScheduleError::OutOfRange),
		_ => Err(ScheduleError::NoOccurrence {
			rule: recurrence.rule.clone(),
			from: date,
		}),
	}
}

/// [`Recurrence::occurrences`], from a new expansion.
fn occurrences_by_new_expansion(
	recurrence: &Recurrence,
	first: Date,
	last: Date,
) -> Vec<Date> {
	fresh(recurrence)
		.map_while(|occurrence| occurrence)
		.skip_while(|&occurrence| occurrence < first)
		.take_while(|&occurrence| occurrence <= last)
		.collect()
}

/// A fixed sequence of pseudo-random numbers below `bound`.
struct Numbers(u64);

impl Numbers {
	fn below(&mut self, bound: u64) -> u64 {
		self.0 = self
			.0
			.wrapping_mul(6_364_136_223_846_793_005)
			.wrapping_add(1_442_695_040_888_963_407);
		(self.0 >> 33) % bound
	}
}

/// `date` moved by `days`, which may be negative, stopping at the first or
/// last date that can be represented.
fn shift(date: Date, days: i64) -> Date {
	date
		.checked_add(Span::new().days(days))
		.unwrap_or(if days < 0 { Date::MIN } else { Date::MAX })
}

/// Searches in any order, on a recurrence and on its clone, find the
/// occurrences a new expansion from DTSTART finds, including past the end of
/// the calendar.
#[test]
fn searches_find_what_a_new_expansion_finds() {
	let mut numbers = Numbers(7);
	let mut cases = 0;
	for rule in RULES {
		for start in STARTS {
			let Some(recurrence) = cadence(rule, start) else {
				continue;
			};
			cases += 1;
			let copy = recurrence.clone();
			let dtstart = recurrence.dtstart();
			for step in 0..40 {
				let searched = if step % 2 == 0 { &recurrence } else { &copy };
				let offset = i64::try_from(numbers.below(1_540)).unwrap() - 40;
				let first = shift(dtstart, offset);
				if numbers.below(2) == 0 {
					assert_eq!(
						searched.next_on_or_after(first),
						next_by_new_expansion(&recurrence, first),
						"{rule} from {dtstart}, next on or after {first}"
					);
				} else {
					let length = i64::try_from(numbers.below(400)).unwrap() - 20;
					let last = shift(first, length);
					assert_eq!(
						searched.occurrences(first, last),
						occurrences_by_new_expansion(&recurrence, first, last),
						"{rule} from {dtstart}, {first} to {last}"
					);
				}
			}
		}
	}
	assert!(cases > 60, "only {cases} cases");
}

/// Two recurrences with the same DTSTART and rule are equal, whatever each
/// has expanded.
#[test]
fn equality_ignores_how_far_a_recurrence_has_expanded() {
	let searched = cadence("FREQ=DAILY", "2026-01-01").unwrap();
	let unsearched = cadence("FREQ=DAILY", "2026-01-01").unwrap();
	searched
		.next_on_or_after("2026-12-31".parse().unwrap())
		.unwrap();
	assert_eq!(searched, unsearched);
	assert_ne!(searched, cadence("FREQ=DAILY", "2026-01-02").unwrap());
	assert_eq!(format!("{searched:?}"), format!("{unsearched:?}"));
}

/// A rule whose first occurrence falls after 31 December 9999 is out of
/// range, rather than naming a DTSTART that cannot be written.
#[test]
fn a_first_occurrence_after_the_year_9999_is_out_of_range() {
	let leap_day = Cadence {
		dtstart: "9998-06-01".parse().unwrap(),
		rrule: "FREQ=YEARLY;BYMONTH=2;BYMONTHDAY=29".to_owned(),
	};
	assert_eq!(Recurrence::new(&leap_day), Err(ScheduleError::OutOfRange));
}

/// A search that reaches past 31 December 9999 is out of range, while
/// earlier occurrences are still found. The dates are written out rather
/// than computed, so the conversions under test do not check themselves.
#[test]
fn a_search_past_the_year_9999_is_out_of_range() {
	let yearly = cadence("FREQ=YEARLY", "9998-06-01").unwrap();
	assert_eq!(
		yearly.next_on_or_after("9999-01-01".parse().unwrap()),
		Ok("9999-06-01".parse().unwrap()),
	);
	assert_eq!(
		yearly.next_on_or_after("9999-06-02".parse().unwrap()),
		Err(ScheduleError::OutOfRange),
	);
	assert_eq!(
		yearly.next_on_or_after("9998-06-01".parse().unwrap()),
		Ok("9998-06-01".parse().unwrap()),
	);
	assert_eq!(
		yearly.occurrences(
			"9998-01-01".parse().unwrap(),
			"9999-12-31".parse().unwrap(),
		),
		["9998-06-01".parse().unwrap(), "9999-06-01".parse().unwrap()],
	);
}

/// The evaluator stops in the year 10 000, so a rule whose next occurrence
/// falls in 10 001 ends the expansion without an unrepresentable date. The
/// search still reaches past 31 December 9999 and is out of range.
#[test]
fn a_gap_past_the_end_of_the_evaluator_is_out_of_range() {
	let biennial = cadence("FREQ=YEARLY;INTERVAL=2", "9999-06-01").unwrap();
	assert_eq!(
		biennial.next_on_or_after("9999-06-02".parse().unwrap()),
		Err(ScheduleError::OutOfRange),
	);
}

/// After a search panics while it holds the shared expansion, searches
/// expand afresh and still agree with the shared expansion's answers.
#[test]
fn a_poisoned_expansion_gives_the_same_answers() {
	let yearly = cadence("FREQ=YEARLY", "9998-06-01").unwrap();
	let poisoned = yearly.clone();
	let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
		poisoned.with_dates(|_| panic!("a search panics"))
	}));
	assert!(poisoned.expansion.is_poisoned());
	for from in ["9998-01-01", "9999-06-01", "9999-06-02"] {
		let from = from.parse().unwrap();
		assert_eq!(
			poisoned.next_on_or_after(from),
			yearly.next_on_or_after(from)
		);
	}
	let (first, last) = ("9998-01-01".parse().unwrap(), Date::MAX);
	assert_eq!(
		poisoned.occurrences(first, last),
		yearly.occurrences(first, last)
	);
}
