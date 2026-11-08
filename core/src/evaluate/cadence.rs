//! Series cadence: windows, credits, overdue and missed.
//!
//! A series with a cadence owes one completed entry per cadence window.
//! [`evaluate`] is a pure function of the cadence, the grace period, the time
//! zone, the entries' completion instants and the evaluation instant. Working
//! out when an entry completed belongs to the caller.
//!
//! Every comparison uses local dates in the configured time zone.
//!
//! # Windows
//!
//! The cadence's occurrences are d₀ (DTSTART), d₁, d₂, and so on. Window k is
//! (d₍ₖ₋₁₎, dₖ + grace]: it contains the local dates after the previous due
//! date, up to and including its own due date plus the grace period. The first
//! window, k = 0, is (−∞, d₀ + grace], so entries completed before the cadence
//! was adopted count for it. Windows overlap when the grace period is longer
//! than zero days.
//!
//! A window is **closed** once the local date of the evaluation instant is
//! after dₖ + grace.
//!
//! # Credits
//!
//! Each entry earns at most one credit. Completions are taken in completion
//! order, and each one credits the earliest window that contains its local
//! date and has no credit yet. A completion whose windows are all credited
//! earns nothing.
//!
//! # Overdue and missed
//!
//! The series is **overdue** when the newest closed window has no credit and
//! no entry has completed since that window closed. A completion after a
//! closed, uncredited window ends the overdue state, and that window becomes
//! **missed**; the completion credits its own, later window (H5). Every older
//! closed window without a credit is missed as well.

#[cfg(test)]
mod tests;

use chrono::{DateTime, NaiveDate, Utc};

use super::schedule::{Recurrence, ScheduleError, TimeZone, add_days};

/// When a series entry completed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Completion<E> {
	/// The entry's identifier.
	pub entry: E,
	/// The instant the entry completed.
	pub at: DateTime<Utc>,
}

/// One cadence window, (`after`, `last_day`], in local dates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Window<E> {
	/// The previous due date, d₍ₖ₋₁₎, which the window excludes, or `None` for
	/// the first window, which has no lower bound.
	pub after: Option<NaiveDate>,
	/// The window's due date, dₖ.
	pub due: NaiveDate,
	/// The last local date the window contains: `due` plus the grace period.
	pub last_day: NaiveDate,
	/// Whether the evaluation instant's local date is after `last_day`.
	pub closed: bool,
	/// The completion credited to this window, if any.
	pub credit: Option<Completion<E>>,
}

impl<E> Window<E> {
	/// Whether the window contains the local date `date`.
	#[must_use]
	pub fn contains(&self, date: NaiveDate) -> bool {
		self.after.is_none_or(|after| after < date) && date <= self.last_day
	}
}

/// The cadence state of a series at one instant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CadenceStatus<E> {
	/// Every window that has opened by the evaluation instant or by the latest
	/// completion, oldest first. A window's position is its index k.
	pub windows: Vec<Window<E>>,
	/// Whether the series is overdue.
	pub overdue: bool,
	/// The positions in `windows` of the missed windows, in ascending order.
	pub missed: Vec<usize>,
}

/// Evaluates a series cadence at `at`, as described in the
/// [module documentation](self).
///
/// `grace` is in whole days, and `zone` converts every instant to a local
/// date. `completions` may come in any order. An entry that appears more
/// than once is considered at its earliest completion only; completions at
/// the same instant keep their order in the slice.
///
/// The windows listed are the first one and every later one whose lower bound
/// precedes the later of the evaluation date and the latest completion date.
/// The last one listed is therefore always open.
///
/// # Errors
///
/// - [`ScheduleError::NoOccurrence`] when the cadence has no occurrence within
///   [`HORIZON_DAYS`](super::schedule::HORIZON_DAYS) of the latest date
///   considered.
/// - [`ScheduleError::OutOfRange`] when a date leaves the supported calendar.
pub fn evaluate<E: Clone + PartialEq>(
	cadence: &Recurrence,
	grace: u32,
	zone: &TimeZone,
	completions: &[Completion<E>],
	at: DateTime<Utc>,
) -> Result<CadenceStatus<E>, ScheduleError> {
	let today = zone.local_date(at)?;
	let completions = in_completion_order(completions, zone)?;
	let latest = completions
		.iter()
		.map(|&(date, _)| date)
		.fold(today, NaiveDate::max);
	let mut windows = cadence_windows(cadence, grace, latest, today)?;

	for &(date, completion) in &completions {
		if let Some(window) = windows
			.iter_mut()
			.find(|window| window.credit.is_none() && window.contains(date))
		{
			window.credit = Some(completion.clone());
		}
	}

	// `last_day` grows with k, so the closed windows come first.
	let newest_closed = windows.iter().rposition(|window| window.closed);
	let overdue = newest_closed.is_some_and(|newest| {
		let window = &windows[newest];
		window.credit.is_none()
			&& completions.iter().all(|&(date, _)| date <= window.last_day)
	});
	let missed = windows
		.iter()
		.enumerate()
		.filter(|&(index, window)| {
			window.closed
				&& window.credit.is_none()
				&& !(overdue && Some(index) == newest_closed)
		})
		.map(|(index, _)| index)
		.collect();

	Ok(CadenceStatus {
		windows,
		overdue,
		missed,
	})
}

/// Each entry's earliest completion with its local date, sorted by instant.
/// The sort is stable, so ties keep their order in `completions`.
fn in_completion_order<'c, E: PartialEq>(
	completions: &'c [Completion<E>],
	zone: &TimeZone,
) -> Result<Vec<(NaiveDate, &'c Completion<E>)>, ScheduleError> {
	let mut ordered: Vec<&Completion<E>> = completions.iter().collect();
	ordered.sort_by_key(|completion| completion.at);
	let mut earliest: Vec<&Completion<E>> = Vec::with_capacity(ordered.len());
	for completion in ordered {
		if !earliest.iter().any(|seen| seen.entry == completion.entry) {
			earliest.push(completion);
		}
	}
	earliest
		.into_iter()
		.map(|completion| Ok((zone.local_date(completion.at)?, completion)))
		.collect()
}

/// The first window and every later one whose lower bound precedes `latest`;
/// the last of them is the first whose due date is on or after `latest`. Each
/// is marked closed or open as of the local date `today`.
fn cadence_windows<E>(
	cadence: &Recurrence,
	grace: u32,
	latest: NaiveDate,
	today: NaiveDate,
) -> Result<Vec<Window<E>>, ScheduleError> {
	let mut due_dates = cadence.occurrences(cadence.dtstart(), latest);
	if due_dates.last() != Some(&latest) {
		due_dates.push(cadence.next_on_or_after(latest)?);
	}
	let mut windows = Vec::with_capacity(due_dates.len());
	let mut after = None;
	for due in due_dates {
		let last_day = add_days(due, grace)?;
		windows.push(Window {
			after,
			due,
			last_day,
			closed: today > last_day,
			credit: None,
		});
		after = Some(due);
	}
	Ok(windows)
}
