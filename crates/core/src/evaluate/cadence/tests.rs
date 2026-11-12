//! Tests for series cadence windows, credits, overdue and missed.

use chrono::{DateTime, NaiveDate, Utc};

use super::{CadenceStatus, Completion, Window, evaluate};
use crate::{
	evaluate::schedule::{Recurrence, TimeZone},
	model::Cadence,
};

fn date(text: &str) -> NaiveDate {
	text.parse().expect("valid test date")
}

fn instant(text: &str) -> DateTime<Utc> {
	text.parse().expect("valid test instant")
}

fn recurrence(dtstart: &str, rrule: &str) -> Recurrence {
	Recurrence::new(&Cadence {
		dtstart: date(dtstart),
		rrule: rrule.to_owned(),
	})
	.expect("valid test cadence")
}

fn done(entry: &str, at: &str) -> Completion<String> {
	Completion {
		entry: entry.to_owned(),
		at: instant(at),
	}
}

fn status(
	cadence: &Recurrence,
	grace: u32,
	completions: &[Completion<String>],
	at: &str,
) -> CadenceStatus<String> {
	evaluate(cadence, grace, &TimeZone::utc(), completions, instant(at))
		.expect("evaluable cadence")
}

/// The entry credited to each window, oldest first.
fn credits(status: &CadenceStatus<String>) -> Vec<Option<&str>> {
	status
		.windows
		.iter()
		.map(|window| window.credit.as_ref().map(|credit| credit.entry.as_str()))
		.collect()
}

#[test]
fn completing_the_day_before_the_due_date_is_on_time() {
	// An audit due on 1 July, completed on 30 June.
	let monthly = recurrence("2026-07-01", "FREQ=MONTHLY");
	let completions = [done("audit-1", "2026-06-30T10:00:00Z")];

	let on_due_date = status(&monthly, 0, &completions, "2026-07-01T12:00:00Z");
	assert_eq!(
		on_due_date,
		CadenceStatus {
			windows: vec![Window {
				after: None,
				due: date("2026-07-01"),
				last_day: date("2026-07-01"),
				closed: false,
				credit: Some(completions[0].clone()),
			}],
			overdue: false,
			missed: vec![],
		}
	);

	let next_day = status(&monthly, 0, &completions, "2026-07-02T00:00:00Z");
	assert_eq!(credits(&next_day), [Some("audit-1"), None]);
	assert!(next_day.windows[0].closed);
	assert!(!next_day.windows[1].closed);
	assert!(!next_day.overdue);
	assert_eq!(next_day.missed, Vec::<usize>::new());
}

#[test]
fn a_late_completion_ends_overdue_and_leaves_the_window_missed() {
	// Monthly, due on the last day of each month.
	let monthly = recurrence("2025-12-31", "FREQ=MONTHLY;BYMONTHDAY=-1");
	let december = done("audit-2025-12", "2025-12-20T10:00:00Z");

	// January's window closes empty.
	let first_of_february = status(
		&monthly,
		0,
		std::slice::from_ref(&december),
		"2026-02-01T09:00:00Z",
	);
	assert_eq!(
		credits(&first_of_february),
		[Some("audit-2025-12"), None, None]
	);
	assert!(first_of_february.windows[1].closed);
	assert!(first_of_february.overdue);
	assert_eq!(first_of_february.missed, Vec::<usize>::new());

	// An audit sealed on 2 February credits February, and January is missed.
	let completions = [december, done("audit-2026-01", "2026-02-02T10:00:00Z")];
	let after_sealing = status(&monthly, 0, &completions, "2026-02-03T09:00:00Z");
	assert_eq!(
		credits(&after_sealing),
		[Some("audit-2025-12"), None, Some("audit-2026-01")]
	);
	assert_eq!(after_sealing.windows[2].due, date("2026-02-28"));
	assert!(!after_sealing.overdue);
	assert_eq!(after_sealing.missed, [1]);
}

#[test]
fn one_late_entry_satisfies_only_one_window() {
	let monthly = recurrence("2026-01-01", "FREQ=MONTHLY");
	let completions = [done("late", "2026-03-15T10:00:00Z")];
	let late = status(&monthly, 0, &completions, "2026-03-16T09:00:00Z");
	assert_eq!(credits(&late), [None, None, None, Some("late")]);
	assert!(!late.overdue);
	assert_eq!(late.missed, [0, 1, 2]);
}

#[test]
fn one_entry_inside_overlapping_windows_credits_only_the_earliest() {
	// Ten days of grace: window 1 is (1 January, 11 February] and window 2 is
	// (1 February, 11 March].
	let monthly = recurrence("2026-01-01", "FREQ=MONTHLY");
	let completions = [done("late", "2026-02-05T10:00:00Z")];
	let after_march = status(&monthly, 10, &completions, "2026-03-12T09:00:00Z");
	assert_eq!(credits(&after_march), [None, Some("late"), None, None]);
	assert!(after_march.windows[2].closed);
	assert!(after_march.overdue);
	assert_eq!(after_march.missed, [0]);
}

#[test]
fn entries_from_before_adoption_count_for_the_first_window() {
	let monthly = recurrence("2026-07-01", "FREQ=MONTHLY");
	let completions = [
		done("old-1", "2025-03-10T10:00:00Z"),
		done("old-2", "2025-04-10T10:00:00Z"),
	];
	let adopted = status(&monthly, 0, &completions, "2026-07-02T09:00:00Z");
	assert_eq!(adopted.windows[0].after, None);
	// The second old entry falls in no other window, so it earns nothing.
	assert_eq!(credits(&adopted), [Some("old-1"), None]);
	assert!(!adopted.overdue);
	assert_eq!(adopted.missed, Vec::<usize>::new());
}

#[test]
fn overlapping_windows_credit_in_completion_order() {
	let monthly = recurrence("2026-01-01", "FREQ=MONTHLY");
	let a = done("a", "2026-01-05T10:00:00Z");
	let b = done("b", "2026-01-08T10:00:00Z");
	let c = done("c", "2026-02-05T10:00:00Z");
	for completions in [
		[a.clone(), b.clone(), c.clone()],
		[c.clone(), b.clone(), a.clone()],
	] {
		let overlapping =
			status(&monthly, 10, &completions, "2026-02-20T09:00:00Z");
		let windows: Vec<_> = overlapping
			.windows
			.iter()
			.map(|window| (window.after, window.due, window.last_day, window.closed))
			.collect();
		assert_eq!(
			windows,
			[
				(None, date("2026-01-01"), date("2026-01-11"), true),
				(
					Some(date("2026-01-01")),
					date("2026-02-01"),
					date("2026-02-11"),
					true
				),
				(
					Some(date("2026-02-01")),
					date("2026-03-01"),
					date("2026-03-11"),
					false
				),
			]
		);
		assert_eq!(credits(&overlapping), [Some("a"), Some("b"), Some("c")]);
		assert!(!overlapping.overdue);
		assert_eq!(overlapping.missed, Vec::<usize>::new());
	}
}

#[test]
fn an_entry_listed_twice_earns_one_credit_at_its_earliest_completion() {
	let monthly = recurrence("2026-01-01", "FREQ=MONTHLY");
	let completions = [
		done("audit", "2026-02-10T10:00:00Z"),
		done("audit", "2026-01-10T10:00:00Z"),
	];
	let listed_twice = status(&monthly, 0, &completions, "2026-02-15T09:00:00Z");
	assert_eq!(credits(&listed_twice), [None, Some("audit"), None]);
	assert_eq!(
		listed_twice.windows[1]
			.credit
			.as_ref()
			.map(|credit| credit.at),
		Some(instant("2026-01-10T10:00:00Z")),
	);
	assert_eq!(listed_twice.missed, [0]);
}

#[test]
fn nothing_is_overdue_before_the_first_window_closes() {
	let monthly = recurrence("2026-12-01", "FREQ=MONTHLY;INTERVAL=2");
	let before = status(&monthly, 3, &[], "2026-12-04T23:59:59Z");
	// The second window opened after DTSTART, so both are listed.
	assert_eq!(before.windows.len(), 2);
	assert!(!before.windows[0].closed);
	assert!(!before.overdue);

	let after = status(&monthly, 3, &[], "2026-12-05T00:00:00Z");
	assert!(after.windows[0].closed);
	assert!(after.overdue);
	assert_eq!(after.missed, Vec::<usize>::new());
}

#[test]
fn completions_and_closings_use_local_dates() {
	// 22:30 UTC on 31 March is 00:30 on 1 April in Madrid.
	let month_ends = recurrence("2026-03-31", "FREQ=MONTHLY;BYMONTHDAY=-1");
	let completions = [done("audit", "2026-03-31T22:30:00Z")];
	let at = instant("2026-04-01T10:00:00Z");

	let madrid = evaluate(
		&month_ends,
		0,
		&TimeZone::new("Europe/Madrid").expect("known time zone"),
		&completions,
		at,
	)
	.expect("evaluable cadence");
	assert_eq!(credits(&madrid), [None, Some("audit")]);
	assert!(!madrid.overdue);
	assert_eq!(madrid.missed, [0]);

	let utc = evaluate(&month_ends, 0, &TimeZone::utc(), &completions, at)
		.expect("evaluable cadence");
	assert_eq!(credits(&utc), [Some("audit"), None]);
	assert_eq!(utc.missed, Vec::<usize>::new());
}

#[test]
fn a_completion_after_the_evaluation_instant_still_counts() {
	// A commit dated up to a day ahead is tolerated.
	let monthly = recurrence("2026-01-01", "FREQ=MONTHLY");
	let completions = [done("audit", "2026-02-03T08:00:00Z")];
	let early = status(&monthly, 0, &completions, "2026-02-02T23:00:00Z");
	assert_eq!(credits(&early), [None, None, Some("audit")]);
	assert!(!early.overdue);
	assert_eq!(early.missed, [0, 1]);
}
