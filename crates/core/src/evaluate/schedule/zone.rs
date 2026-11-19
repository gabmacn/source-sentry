//! Time zones named by IANA identifiers, resolved against a bundled copy of
//! the IANA time zone database.

use std::{fmt, str::FromStr};

use jiff::{
	Timestamp, Zoned,
	civil::{Date, Time},
	tz::{AmbiguousOffset, TimeZoneDatabase},
};
use serde::{Deserialize, Serialize};

use super::ScheduleError;

/// A time zone, named by its IANA identifier, such as `Europe/Madrid`. The
/// default is `UTC`.
///
/// Names are looked up in the copy of the IANA time zone database that is
/// compiled into Source Sentry, never in the operating system's, so every
/// platform applies the same rules. A name must match the database exactly,
/// including case. Fixed offsets such as `+01:00` are not names and are
/// rejected; names from the database's `Etc` area, such as `Etc/GMT-1`, are
/// accepted like any other.
///
/// Two time zones are equal when their names are.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct TimeZone {
	name: String,
	zone: jiff::tz::TimeZone,
}

impl TimeZone {
	/// Coordinated Universal Time, the default.
	#[must_use]
	pub fn utc() -> Self {
		Self {
			name: "UTC".to_owned(),
			zone: jiff::tz::TimeZone::UTC,
		}
	}

	/// The time zone with the IANA name `name`.
	///
	/// # Errors
	///
	/// Fails with [`ScheduleError::UnknownTimeZone`] when the bundled database
	/// has no zone with exactly that name.
	pub fn new(name: &str) -> Result<Self, ScheduleError> {
		let unknown = || ScheduleError::UnknownTimeZone(name.to_owned());
		let zone = TimeZoneDatabase::bundled()
			.get(name)
			.map_err(|_| unknown())?;
		// The database ignores ASCII case, and answers `Etc/Unknown` with a
		// zone that has no IANA name. Neither is a configured IANA name.
		if zone.iana_name() != Some(name) {
			return Err(unknown());
		}
		Ok(Self {
			name: name.to_owned(),
			zone,
		})
	}

	/// The IANA name, as configured.
	#[must_use]
	pub fn name(&self) -> &str {
		&self.name
	}

	/// The local date at `instant`.
	#[must_use]
	pub fn local_date(&self, instant: Timestamp) -> Date {
		self.zone.to_datetime(instant).date()
	}

	/// The first instant of the local day `date`.
	///
	/// This is the instant at which local clocks read midnight. When midnight
	/// occurs twice, it is the earlier instant. When a transition skips
	/// midnight, it is the instant of that transition, the first one after the
	/// skipped midnight. When a transition skips the whole day, as in
	/// `Pacific/Apia` on 30 December 2011, it is the first instant of the
	/// following day.
	///
	/// # Errors
	///
	/// Fails with [`ScheduleError::OutOfRange`] when that instant lies outside
	/// the supported range, `-009999-01-02T01:59:59Z` to
	/// `9999-12-30T22:00:00.999999999Z`.
	pub fn start_of_day(&self, date: Date) -> Result<Timestamp, ScheduleError> {
		let midnight = date.to_datetime(Time::midnight());
		let start = match self.zone.to_ambiguous_timestamp(midnight).offset() {
			AmbiguousOffset::Unambiguous { offset } => offset.to_timestamp(midnight),
			// `before` is the offset in force before the clocks fall back, which
			// reads midnight first.
			AmbiguousOffset::Fold { before, .. } => before.to_timestamp(midnight),
			AmbiguousOffset::Gap { after, .. } => {
				// The clocks jump over midnight at a transition T. The instant at
				// which the later offset would read midnight precedes T, and T is
				// the first transition after that instant whose new local time is
				// on or after midnight.
				let bound = after
					.to_timestamp(midnight)
					.map_err(|_| ScheduleError::OutOfRange)?;
				let transition = self
					.zone
					.following(bound)
					.map(|transition| transition.timestamp())
					.find(|&instant| self.zone.to_datetime(instant) >= midnight);
				// A gap always has a transition; this guards against corrupt data.
				return transition.ok_or(ScheduleError::OutOfRange);
			}
		};
		start.map_err(|_| ScheduleError::OutOfRange)
	}

	/// `instant` as a local date and time in this zone, for display.
	#[must_use]
	pub fn to_local(&self, instant: Timestamp) -> Zoned {
		instant.to_zoned(self.zone.clone())
	}
}

impl Default for TimeZone {
	fn default() -> Self {
		Self::utc()
	}
}

impl PartialEq for TimeZone {
	fn eq(&self, other: &Self) -> bool {
		self.name == other.name
	}
}

impl Eq for TimeZone {}

impl fmt::Display for TimeZone {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		formatter.write_str(&self.name)
	}
}

impl FromStr for TimeZone {
	type Err = ScheduleError;

	fn from_str(name: &str) -> Result<Self, Self::Err> {
		Self::new(name)
	}
}

impl TryFrom<String> for TimeZone {
	type Error = ScheduleError;

	fn try_from(name: String) -> Result<Self, Self::Error> {
		Self::new(&name)
	}
}

impl From<TimeZone> for String {
	fn from(zone: TimeZone) -> Self {
		zone.name
	}
}
