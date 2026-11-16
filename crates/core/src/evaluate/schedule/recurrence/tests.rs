//! Tests that searches over the shared expansion find what a new expansion
//! from DTSTART finds.

use chrono::{Days, NaiveDate};

use super::{HORIZON_DAYS, Recurrence};
use crate::evaluate::schedule::ScheduleError;
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
/// has none there.
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
		Err(ScheduleError::NoOccurrence { .. }) => None,
		Err(error) => panic!("{rule} from {start}: {error}"),
	}
}

/// Every occurrence from DTSTART on, from a new expansion.
fn fresh(recurrence: &Recurrence) -> impl Iterator<Item = NaiveDate> + '_ {
	(&recurrence.set)
		.into_iter()
		.map(|occurrence| occurrence.date_naive())
}

/// [`Recurrence::next_on_or_after`], from a new expansion.
fn next_by_new_expansion(
	recurrence: &Recurrence,
	date: NaiveDate,
) -> Result<NaiveDate, ScheduleError> {
	let limit = date
		.checked_add_days(Days::new(HORIZON_DAYS))
		.unwrap_or(NaiveDate::MAX);
	match fresh(recurrence).find(|&occurrence| occurrence >= date) {
		Some(occurrence) if occurrence <= limit => Ok(occurrence),
		_ => Err(ScheduleError::NoOccurrence {
			rule: recurrence.rule.clone(),
			from: date,
		}),
	}
}

/// [`Recurrence::occurrences`], from a new expansion.
fn occurrences_by_new_expansion(
	recurrence: &Recurrence,
	first: NaiveDate,
	last: NaiveDate,
) -> Vec<NaiveDate> {
	fresh(recurrence)
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

/// `date` moved by `days`, which may be negative.
fn shift(date: NaiveDate, days: i64) -> NaiveDate {
	let step = Days::new(days.unsigned_abs());
	if days < 0 {
		date.checked_sub_days(step).unwrap()
	} else {
		date.checked_add_days(step).unwrap()
	}
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
