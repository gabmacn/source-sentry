//! Tests for choosing a series' next deadline from its evaluated windows.

use chrono::{Days, NaiveDate};

use super::next_deadline;
use crate::evaluate::cadence::{CadenceStatus, Completion, Window};
use crate::evaluate::schedule::{Deadline, Recurrence, TimeZone};
use crate::model::{Cadence, RepoPath};

/// The first due date of every daily status built here.
fn start() -> NaiveDate {
	NaiveDate::from_ymd_opt(1500, 1, 1).unwrap()
}

/// A daily cadence from [`start`].
fn daily() -> Recurrence {
	Recurrence::new(&Cadence {
		dtstart: start(),
		rrule: "FREQ=DAILY".to_owned(),
	})
	.unwrap()
}

/// What happened to one window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
	Credited,
	Missed,
	Neither,
}

/// A daily status with one window per state, all closed except the last.
fn status(states: &[State]) -> CadenceStatus<RepoPath> {
	let mut windows = Vec::with_capacity(states.len());
	let mut missed = Vec::new();
	let mut after = None;
	for (index, &state) in states.iter().enumerate() {
		let due = start()
			.checked_add_days(Days::new(index.try_into().unwrap()))
			.unwrap();
		let credit = (state == State::Credited).then(|| Completion {
			entry: RepoPath::new(&format!("audit-{index}.md")).unwrap(),
			at: due.and_hms_opt(12, 0, 0).unwrap().and_utc(),
		});
		if state == State::Missed {
			missed.push(index);
		}
		windows.push(Window {
			after,
			due,
			last_day: due,
			closed: index + 1 < states.len(),
			credit,
		});
		after = Some(due);
	}
	CadenceStatus {
		windows,
		overdue: false,
		missed,
	}
}

/// The deadline found by testing each window against the whole missed list.
fn deadline_by_membership(
	status: &CadenceStatus<RepoPath>,
	recurrence: &Recurrence,
	zone: &TimeZone,
) -> Option<Deadline> {
	let open = status
		.windows
		.iter()
		.enumerate()
		.find(|(index, window)| {
			window.credit.is_none() && !status.missed.contains(index)
		})
		.map(|(_, window)| window.last_day);
	let last_day = match (open, status.windows.last()) {
		(Some(last_day), _) => last_day,
		(None, Some(last)) => recurrence
			.next_on_or_after(last.due.checked_add_days(Days::new(1)).unwrap())
			.unwrap(),
		(None, None) => return None,
	};
	Some(Deadline {
		due: Some(last_day),
		due_from: zone.start_of_day(last_day).unwrap(),
		overdue_from: zone
			.start_of_day(last_day.checked_add_days(Days::new(1)).unwrap())
			.unwrap(),
		provisional: false,
	})
}

/// A backlog of missed windows ahead of the open one yields the open
/// window's deadline, as it does when the same windows were credited.
#[test]
fn a_long_missed_backlog_yields_the_open_windows_deadline() {
	let recurrence = daily();
	let zone = TimeZone::utc();
	for count in [3, 160_001] {
		let mut states = vec![State::Missed; count - 1];
		states.push(State::Neither);
		let missed = status(&states);
		let open = missed.windows[count - 1].due;

		let actual = next_deadline(&missed, &recurrence, 0, &zone).unwrap();
		assert_eq!(actual.and_then(|deadline| deadline.due), Some(open));

		states.fill(State::Credited);
		states[count - 1] = State::Neither;
		let credited = status(&states);
		assert_eq!(
			next_deadline(&credited, &recurrence, 0, &zone).unwrap(),
			actual
		);
	}
}

/// For every arrangement of credited, missed and other windows up to eight
/// long, the deadline is the one a membership search of the missed list
/// finds.
#[test]
fn the_deadline_matches_a_membership_search_for_every_small_status() {
	const STATES: [State; 3] = [State::Credited, State::Missed, State::Neither];
	let recurrence = daily();
	let zone = TimeZone::utc();
	for length in 0..=8_u32 {
		for code in 0..3_usize.pow(length) {
			let mut rest = code;
			let arrangement: Vec<State> = (0..length)
				.map(|_| {
					let state = STATES[rest % 3];
					rest /= 3;
					state
				})
				.collect();
			let status = status(&arrangement);
			assert_eq!(
				next_deadline(&status, &recurrence, 0, &zone).unwrap(),
				deadline_by_membership(&status, &recurrence, &zone),
				"{arrangement:?}"
			);
		}
	}
}
