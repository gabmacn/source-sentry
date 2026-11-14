//! Tests for cadences, time zones and deadlines.

use chrono::{DateTime, Duration, NaiveDate, Timelike, Utc};

use super::{Deadline, EffectiveSchedule, Recurrence, ScheduleError, TimeZone};
use crate::model::{Cadence, CadenceSetting, Schedule};

fn date(text: &str) -> NaiveDate {
	text.parse().expect("valid test date")
}

fn dates(texts: &[&str]) -> Vec<NaiveDate> {
	texts.iter().map(|text| date(text)).collect()
}

fn instant(text: &str) -> DateTime<Utc> {
	text.parse().expect("valid test instant")
}

fn cadence(dtstart: &str, rrule: &str) -> Cadence {
	Cadence {
		dtstart: date(dtstart),
		rrule: rrule.to_owned(),
	}
}

fn recurrence(dtstart: &str, rrule: &str) -> Recurrence {
	Recurrence::new(&cadence(dtstart, rrule)).expect("valid test cadence")
}

fn rejection(dtstart: &str, rrule: &str) -> ScheduleError {
	Recurrence::new(&cadence(dtstart, rrule))
		.expect_err("the cadence must be rejected")
}

fn zone(name: &str) -> TimeZone {
	TimeZone::new(name).expect("known test time zone")
}

fn schedule(grace: u32, cadence: Option<(&str, &str)>) -> EffectiveSchedule {
	EffectiveSchedule {
		grace,
		cadence: cadence.map(|(dtstart, rrule)| recurrence(dtstart, rrule)),
	}
}

fn deadline(
	schedule: &EffectiveSchedule,
	start: &str,
	zone_name: &str,
) -> Deadline {
	schedule
		.deadline(instant(start), &zone(zone_name))
		.expect("computable deadline")
}

// Recurrences.

#[test]
fn weekly_rule_falls_on_each_monday() {
	let weekly = recurrence("2026-12-07", "FREQ=WEEKLY;BYDAY=MO");
	assert_eq!(
		weekly.occurrences(date("2026-12-01"), date("2027-01-04")),
		dates(&[
			"2026-12-07",
			"2026-12-14",
			"2026-12-21",
			"2026-12-28",
			"2027-01-04"
		]),
	);
}

#[test]
fn monthly_rule_with_interval_two_follows_dtstart() {
	let bimonthly = recurrence("2026-12-01", "FREQ=MONTHLY;INTERVAL=2");
	assert_eq!(
		bimonthly.occurrences(date("2026-11-01"), date("2027-06-30")),
		dates(&["2026-12-01", "2027-02-01", "2027-04-01", "2027-06-01"]),
	);
	assert_eq!(
		bimonthly.next_on_or_after(date("2027-01-15")),
		Ok(date("2027-02-01")),
	);
}

#[test]
fn leap_day_rule_skips_years_without_29_february() {
	let leap_day =
		recurrence("2096-02-29", "FREQ=YEARLY;BYMONTH=2;BYMONTHDAY=29");
	// 2100 is not a leap year, so the gap is eight years.
	assert_eq!(
		leap_day.next_on_or_after(date("2096-03-01")),
		Ok(date("2104-02-29")),
	);
	assert_eq!(
		leap_day.occurrences(date("2096-01-01"), date("2108-12-31")),
		dates(&["2096-02-29", "2104-02-29", "2108-02-29"]),
	);
}

#[test]
fn numbered_weekdays_and_set_positions_are_supported() {
	let first_monday = recurrence("2026-12-07", "FREQ=MONTHLY;BYDAY=1MO");
	assert_eq!(
		first_monday.next_on_or_after(date("2026-12-08")),
		Ok(date("2027-01-04")),
	);
	let last_weekday_pair =
		recurrence("2026-01-27", "FREQ=MONTHLY;BYDAY=MO,TU;BYSETPOS=-1");
	assert_eq!(
		last_weekday_pair.occurrences(date("2026-01-01"), date("2026-03-31")),
		dates(&["2026-01-27", "2026-02-24", "2026-03-31"]),
	);
}

#[test]
fn searches_start_at_dtstart_and_include_their_own_date() {
	let monthly = recurrence("2026-12-01", "FREQ=MONTHLY");
	assert_eq!(monthly.dtstart(), date("2026-12-01"));
	assert_eq!(monthly.rule(), "FREQ=MONTHLY");
	assert_eq!(
		monthly.next_on_or_after(date("2020-01-01")),
		Ok(date("2026-12-01")),
	);
	assert_eq!(
		monthly.next_on_or_after(date("2027-01-01")),
		Ok(date("2027-01-01")),
	);
	assert_eq!(
		monthly.occurrences(date("2026-01-01"), date("2026-11-30")),
		dates(&[])
	);
	assert_eq!(
		monthly.occurrences(date("2027-03-01"), date("2027-01-01")),
		dates(&[])
	);
}

#[test]
fn time_of_day_parts_are_rejected() {
	for (rrule, part) in [
		("FREQ=DAILY;BYHOUR=9", "BYHOUR"),
		("FREQ=DAILY;BYMINUTE=30", "BYMINUTE"),
		("FREQ=DAILY;BYSECOND=0", "BYSECOND"),
		("FREQ=HOURLY", "FREQ=HOURLY"),
		("FREQ=MINUTELY", "FREQ=MINUTELY"),
		("FREQ=SECONDLY", "FREQ=SECONDLY"),
	] {
		assert_eq!(
			rejection("2026-12-01", rrule),
			ScheduleError::TimeOfDay {
				rule: rrule.to_owned(),
				part,
			},
		);
	}
}

#[test]
fn count_and_until_are_rejected() {
	for (rrule, part) in [
		("FREQ=MONTHLY;COUNT=12", "COUNT"),
		("FREQ=MONTHLY;UNTIL=20271201T000000Z", "UNTIL"),
		("FREQ=MONTHLY;UNTIL=20271201", "UNTIL"),
	] {
		assert_eq!(
			rejection("2026-12-01", rrule),
			ScheduleError::Bounded {
				rule: rrule.to_owned(),
				part,
			},
		);
	}
}

#[test]
fn a_rule_without_occurrences_is_rejected() {
	for rrule in [
		"FREQ=YEARLY;BYMONTH=2;BYMONTHDAY=30",
		"FREQ=MONTHLY;BYMONTH=2;BYMONTHDAY=30",
		"FREQ=DAILY;BYMONTH=4;BYMONTHDAY=31",
	] {
		assert_eq!(
			rejection("2026-01-01", rrule),
			ScheduleError::NoOccurrence {
				rule: rrule.to_owned(),
				from: date("2026-01-01"),
			},
		);
	}
}

#[test]
fn dtstart_must_be_the_first_occurrence() {
	// 5 November 2026 is a Thursday.
	assert_eq!(
		rejection("2026-11-05", "FREQ=WEEKLY;BYDAY=WE"),
		ScheduleError::DtstartNotOccurrence {
			rule: "FREQ=WEEKLY;BYDAY=WE".to_owned(),
			dtstart: date("2026-11-05"),
			first: date("2026-11-11"),
		},
	);
}

#[test]
fn malformed_rules_are_rejected() {
	for rrule in [
		"",
		"BYDAY=MO",
		"FREQ=FORTNIGHTLY",
		"FREQ=WEEKLY;BYDAY=MOO",
		"FREQ=WEEKLY;X-NAME=1",
		"FREQ=WEEKLY;BYEASTER=0",
		"RRULE:FREQ=WEEKLY",
		"EXRULE:FREQ=WEEKLY",
		"FREQ=WEEKLY\nFREQ=DAILY",
		"FREQ=DAILY;BY\u{017f}ECOND=0",
		"FREQ=DAILY;UNT\u{0131}L=20271201T000000Z",
		"FREQ=DAILY;WK\u{017f}T=MO",
	] {
		assert!(
			matches!(
				rejection("2026-11-02", rrule),
				ScheduleError::InvalidRule { .. }
			),
			"{rrule:?} must be invalid",
		);
	}
}

#[test]
fn combinations_that_rfc_5545_forbids_are_rejected() {
	for rrule in [
		"FREQ=MONTHLY;INTERVAL=0",
		"FREQ=WEEKLY;BYDAY=1MO",
		"FREQ=DAILY;BYDAY=1MO",
		"FREQ=YEARLY;BYWEEKNO=2;BYDAY=1MO",
		"FREQ=WEEKLY;BYMONTHDAY=-1",
		"FREQ=MONTHLY;BYSETPOS=1",
		"FREQ=DAILY;BYYEARDAY=1",
		"FREQ=MONTHLY;BYWEEKNO=1",
	] {
		assert!(
			matches!(
				rejection("2026-01-05", rrule),
				ScheduleError::InvalidRule { .. }
			),
			"{rrule:?} must be invalid",
		);
	}
}

/// Non-ASCII weekday tokens are rejected without entering the parser's slices.
#[test]
fn non_ascii_weekdays_are_invalid_rules() {
	for value in ["日", "💥", "1日", "éSU", "SU,💥"] {
		let rule = format!("FREQ=MONTHLY;BYDAY={value}");
		assert!(matches!(
			rejection("2026-11-01", &rule),
			ScheduleError::InvalidRule { .. }
		));
	}
}

/// Malformed and overflowing ordinal prefixes must not become plain weekdays.
#[test]
fn malformed_weekday_ordinals_are_invalid_rules() {
	for value in [
		"garbageSU",
		"999999999999SU",
		"+SU",
		"-SU",
		"--1SU",
		"1.0SU",
		"1 SU",
		"0001SU",
		"54SU",
		"-54SU",
	] {
		let rule = format!("FREQ=MONTHLY;BYDAY={value}");
		assert!(
			matches!(
				rejection("2026-11-01", &rule),
				ScheduleError::InvalidRule { .. }
			),
			"{rule:?} must be invalid"
		);
	}
}

/// Zero selectors, including signed forms and list members, are invalid.
#[test]
fn zero_recurrence_selectors_are_invalid_rules() {
	for part in [
		"BYMONTHDAY=0",
		"BYMONTHDAY=+0",
		"BYMONTHDAY=-0",
		"BYMONTHDAY=1,0",
		"BYDAY=0SU",
		"BYDAY=+0SU",
		"BYDAY=-0SU",
		"BYDAY=1SU,0MO",
		"BYWEEKDAY=0SU",
		"byday=0su",
		"BYYEARDAY=0",
		"BYWEEKNO=0",
		"BYDAY=SU;BYSETPOS=0",
	] {
		let rule = format!("FREQ=YEARLY;{part}");
		assert!(
			matches!(
				rejection("2026-11-01", &rule),
				ScheduleError::InvalidRule { .. }
			),
			"{rule:?} must be invalid"
		);
	}
}

/// Empty properties and separators cannot silently disappear during parsing.
#[test]
fn empty_recurrence_components_are_invalid_rules() {
	for rule in [
		"FREQ=MONTHLY;BYMONTHDAY=",
		"FREQ=YEARLY;BYYEARDAY=",
		"FREQ=YEARLY;BYWEEKNO=",
		"FREQ=MONTHLY;BYMONTH=",
		"FREQ=MONTHLY;BYDAY=SU;BYSETPOS=",
		"FREQ=MONTHLY;",
		";FREQ=MONTHLY",
		"FREQ=MONTHLY;;BYDAY=SU",
		"FREQ=MONTHLY;BYDAY=SU,",
		"FREQ=MONTHLY;BYMONTHDAY=,1",
	] {
		assert!(
			matches!(
				rejection("2026-11-01", rule),
				ScheduleError::InvalidRule { .. }
			),
			"{rule:?} must be invalid"
		);
	}
}

/// A time property is forbidden even when it has no value or invalid syntax.
#[test]
fn time_properties_are_rejected_by_presence() {
	for part in ["BYHOUR", "BYMINUTE", "BYSECOND"] {
		for value in ["", "garbage", "0"] {
			let rule = format!("FREQ=DAILY;{}={value}", part.to_lowercase());
			assert_eq!(
				rejection("2026-11-01", &rule),
				ScheduleError::TimeOfDay { rule, part }
			);
		}
	}
}

/// Bounds are forbidden even when their values are empty or malformed.
#[test]
fn recurrence_bounds_are_rejected_by_presence() {
	for part in ["COUNT", "UNTIL"] {
		for value in ["", "garbage"] {
			let rule = format!("FREQ=DAILY;{}={value}", part.to_lowercase());
			assert_eq!(
				rejection("2026-11-01", &rule),
				ScheduleError::Bounded { rule, part }
			);
		}
	}
}

/// Signed ordinals and case-insensitive weekday names retain their meaning.
#[test]
fn valid_weekday_spellings_keep_their_occurrences() {
	for (part, next) in [
		("BYDAY=SU", "2026-11-08"),
		("byday=su", "2026-11-08"),
		("BYWEEKDAY=SU", "2026-11-08"),
		("BYDAY=+1SU", "2026-12-06"),
		("BYDAY=01SU", "2026-12-06"),
	] {
		let rule = format!("FREQ=MONTHLY;{part}");
		assert_eq!(
			recurrence("2026-11-01", &rule).next_on_or_after(date("2026-11-02")),
			Ok(date(next))
		);
	}
}

#[test]
fn mixing_plain_and_numbered_weekdays_is_unsupported() {
	assert!(matches!(
		rejection("2026-01-02", "FREQ=MONTHLY;BYDAY=FR,1FR"),
		ScheduleError::UnsupportedRule { .. }
	));
}

// Time zones.

#[test]
fn the_default_time_zone_is_utc() {
	assert_eq!(TimeZone::default(), TimeZone::utc());
	assert_eq!(TimeZone::default().name(), "UTC");
}

#[test]
fn iana_names_are_accepted_exactly() {
	for name in [
		"UTC",
		"Etc/UTC",
		"Europe/Madrid",
		"America/Sao_Paulo",
		"Etc/GMT-1",
	] {
		assert_eq!(zone(name).name(), name);
	}
	for name in [
		"",
		"+01:00",
		"UTC+01:00",
		"CET-1CEST",
		"europe/madrid",
		"Etc/Unknown",
		"Mars/Olympus_Mons",
	] {
		assert_eq!(
			TimeZone::new(name),
			Err(ScheduleError::UnknownTimeZone(name.to_owned())),
		);
	}
}

#[test]
fn time_zones_read_and_write_as_names() {
	let madrid: TimeZone =
		serde_json::from_str("\"Europe/Madrid\"").expect("a valid name");
	assert_eq!(madrid, zone("Europe/Madrid"));
	assert_eq!(
		serde_json::to_string(&madrid).expect("serializable"),
		"\"Europe/Madrid\""
	);
	assert!(serde_json::from_str::<TimeZone>("\"+01:00\"").is_err());
	assert_eq!("UTC".parse(), Ok(TimeZone::utc()));
	assert_eq!(zone("Europe/Madrid").to_string(), "Europe/Madrid");
}

#[test]
fn local_dates_follow_the_time_zone() {
	let late_evening = instant("2026-03-28T23:30:00Z");
	assert_eq!(
		zone("Europe/Madrid").local_date(late_evening),
		Ok(date("2026-03-29")),
	);
	assert_eq!(
		TimeZone::utc().local_date(late_evening),
		Ok(date("2026-03-28"))
	);
	assert_eq!(
		zone("America/Sao_Paulo").local_date(instant("2026-03-29T02:00:00Z")),
		Ok(date("2026-03-28")),
	);
}

#[test]
fn days_start_at_local_midnight_across_daylight_saving_time() {
	let madrid = zone("Europe/Madrid");
	// Central European Time, then Central European Summer Time.
	assert_eq!(
		madrid.start_of_day(date("2026-03-29")),
		Ok(instant("2026-03-28T23:00:00Z")),
	);
	assert_eq!(
		madrid.start_of_day(date("2026-03-30")),
		Ok(instant("2026-03-29T22:00:00Z")),
	);
	assert_eq!(
		madrid.start_of_day(date("2026-10-26")),
		Ok(instant("2026-10-25T23:00:00Z")),
	);
}

#[test]
fn a_skipped_midnight_starts_the_day_at_the_transition() {
	// São Paulo sprang forward from 00:00 to 01:00 on 4 November 2018.
	assert_eq!(
		zone("America/Sao_Paulo").start_of_day(date("2018-11-04")),
		Ok(instant("2018-11-04T03:00:00Z")),
	);
	// Havana sprang forward from 00:00 to 01:00 on 9 March 2025.
	assert_eq!(
		zone("America/Havana").start_of_day(date("2025-03-09")),
		Ok(instant("2025-03-09T05:00:00Z")),
	);
	// Apia skipped 30 December 2011 entirely.
	assert_eq!(
		zone("Pacific/Apia").start_of_day(date("2011-12-30")),
		Ok(instant("2011-12-30T10:00:00Z")),
	);
}

#[test]
fn a_repeated_midnight_starts_the_day_at_its_first_occurrence() {
	// Havana fell back from 01:00 to 00:00 on 2 November 2025, so midnight
	// occurred at -04:00 and again at -05:00.
	assert_eq!(
		zone("America/Havana").start_of_day(date("2025-11-02")),
		Ok(instant("2025-11-02T04:00:00Z")),
	);
}

#[test]
fn instants_display_with_the_local_offset() {
	let local = zone("Europe/Madrid")
		.to_local(instant("2026-07-01T12:00:00Z"))
		.expect("in range");
	assert_eq!(local.offset().local_minus_utc(), 2 * 3600);
	assert_eq!(local.hour(), 14);
}

#[test]
fn instants_beyond_the_supported_calendar_are_errors() {
	assert_eq!(
		TimeZone::utc().local_date(DateTime::<Utc>::MAX_UTC),
		Err(ScheduleError::OutOfRange),
	);
	assert_eq!(
		TimeZone::utc().start_of_day(NaiveDate::MAX),
		Err(ScheduleError::OutOfRange),
	);
}

// Effective schedules and deadlines.

#[test]
fn an_unset_grace_period_is_zero_days() {
	let effective = EffectiveSchedule::new(&Schedule {
		cadence: CadenceSetting::Inherit,
		grace: None,
	})
	.expect("valid schedule");
	assert_eq!(effective, EffectiveSchedule::default());
	assert_eq!(effective.grace, 0);
	assert!(effective.cadence.is_none());
}

#[test]
fn a_schedule_validates_its_cadence() {
	let effective = EffectiveSchedule::new(&Schedule {
		cadence: CadenceSetting::Set(cadence(
			"2026-12-01",
			"FREQ=MONTHLY;INTERVAL=2",
		)),
		grace: Some(14),
	})
	.expect("valid schedule");
	assert_eq!(effective.grace, 14);
	assert_eq!(
		effective.cadence.map(|cadence| cadence.dtstart()),
		Some(date("2026-12-01")),
	);
	assert!(matches!(
		EffectiveSchedule::new(&Schedule {
			cadence: CadenceSetting::Set(cadence(
				"2026-12-01",
				"FREQ=DAILY;BYHOUR=9"
			)),
			grace: None,
		}),
		Err(ScheduleError::TimeOfDay { .. })
	));
}

#[test]
fn zero_grace_without_a_cadence_is_due_at_once() {
	let start = instant("2026-11-07T15:20:00Z");
	let due = EffectiveSchedule::default()
		.deadline(start, &TimeZone::utc())
		.expect("computable deadline");
	assert_eq!(
		due,
		Deadline {
			due: None,
			due_from: start,
			overdue_from: start,
			provisional: false,
		}
	);
	assert!(due.is_overdue(start));
	assert!(!due.is_overdue(start - Duration::seconds(1)));
	assert!(due.is_due(start));
	assert!(!due.is_due(start - Duration::seconds(1)));
}

#[test]
fn zero_grace_with_a_cadence_may_be_due_on_the_start_date() {
	// 2 and 9 November 2026 are Mondays.
	let weekly = schedule(0, Some(("2026-11-02", "FREQ=WEEKLY;BYDAY=MO")));
	assert_eq!(
		deadline(&weekly, "2026-11-09T10:00:00Z", "UTC"),
		Deadline {
			due: Some(date("2026-11-09")),
			due_from: instant("2026-11-09T00:00:00Z"),
			overdue_from: instant("2026-11-10T00:00:00Z"),
			provisional: false,
		}
	);
	assert_eq!(
		deadline(&weekly, "2026-11-10T10:00:00Z", "UTC").due,
		Some(date("2026-11-16")),
	);
}

#[test]
fn grace_without_a_cadence_ends_with_its_last_day() {
	let three_days = schedule(3, None);
	let due = deadline(&three_days, "2026-11-07T15:20:00Z", "UTC");
	assert_eq!(
		due,
		Deadline {
			due: Some(date("2026-11-10")),
			due_from: instant("2026-11-10T00:00:00Z"),
			overdue_from: instant("2026-11-11T00:00:00Z"),
			provisional: false,
		}
	);
	assert!(!due.is_overdue(instant("2026-11-10T23:59:59Z")));
	assert!(due.is_overdue(instant("2026-11-11T00:00:00Z")));
	assert!(!due.is_due(instant("2026-11-09T23:59:59Z")));
	assert!(due.is_due(instant("2026-11-10T00:00:00Z")));
}

#[test]
fn grace_then_cadence_picks_the_first_occurrence_after_the_grace() {
	let bimonthly = schedule(14, Some(("2026-12-01", "FREQ=MONTHLY;INTERVAL=2")));
	assert_eq!(
		deadline(&bimonthly, "2026-11-07T09:00:00Z", "UTC"),
		Deadline {
			due: Some(date("2026-12-01")),
			due_from: instant("2026-12-01T00:00:00Z"),
			overdue_from: instant("2026-12-02T00:00:00Z"),
			provisional: false,
		}
	);
	// A change shortly before a cadence date is not due the next day.
	let short_grace =
		schedule(3, Some(("2026-12-01", "FREQ=MONTHLY;INTERVAL=2")));
	assert_eq!(
		deadline(&short_grace, "2027-01-31T09:00:00Z", "UTC").due,
		Some(date("2027-04-01")),
	);
}

#[test]
fn deadlines_in_madrid_cross_daylight_saving_time() {
	let one_day = schedule(1, None);
	// Spring forward on 29 March 2026: due day in CET, overdue in CEST.
	assert_eq!(
		deadline(&one_day, "2026-03-28T12:00:00Z", "Europe/Madrid"),
		Deadline {
			due: Some(date("2026-03-29")),
			due_from: instant("2026-03-28T23:00:00Z"),
			overdue_from: instant("2026-03-29T22:00:00Z"),
			provisional: false,
		}
	);
	// Fall back on 25 October 2026.
	assert_eq!(
		deadline(&one_day, "2026-10-24T12:00:00Z", "Europe/Madrid"),
		Deadline {
			due: Some(date("2026-10-25")),
			due_from: instant("2026-10-24T22:00:00Z"),
			overdue_from: instant("2026-10-25T23:00:00Z"),
			provisional: false,
		}
	);
	// The start's local date is already 29 March in Madrid.
	assert_eq!(
		deadline(&one_day, "2026-03-28T23:30:00Z", "Europe/Madrid"),
		Deadline {
			due: Some(date("2026-03-30")),
			due_from: instant("2026-03-29T22:00:00Z"),
			overdue_from: instant("2026-03-30T22:00:00Z"),
			provisional: false,
		}
	);
	assert_eq!(
		deadline(&one_day, "2026-03-28T23:30:00Z", "UTC").due,
		Some(date("2026-03-29")),
	);
}

#[test]
fn deadlines_in_sao_paulo_cross_daylight_saving_time() {
	// Midnight of 4 November 2018 did not exist: the day began at 01:00 -02,
	// which is 03:00 UTC.
	let one_day = schedule(1, None);
	assert_eq!(
		deadline(&one_day, "2018-11-02T15:00:00Z", "America/Sao_Paulo"),
		Deadline {
			due: Some(date("2018-11-03")),
			due_from: instant("2018-11-03T03:00:00Z"),
			overdue_from: instant("2018-11-04T03:00:00Z"),
			provisional: false,
		}
	);
	// 27 October and 3 November 2018 are Saturdays.
	let saturdays = schedule(0, Some(("2018-10-27", "FREQ=WEEKLY;BYDAY=SA")));
	assert_eq!(
		deadline(&saturdays, "2018-10-29T12:00:00Z", "America/Sao_Paulo"),
		Deadline {
			due: Some(date("2018-11-03")),
			due_from: instant("2018-11-03T03:00:00Z"),
			overdue_from: instant("2018-11-04T03:00:00Z"),
			provisional: false,
		}
	);
	// On 17 February 2019 the clocks fell back from 00:00 to 23:00, so 23:30
	// on the 16th happened twice; both belong to the 16th.
	let daily = schedule(0, Some(("2019-01-01", "FREQ=DAILY")));
	for start in ["2019-02-16T12:00:00Z", "2019-02-17T02:30:00Z"] {
		assert_eq!(
			deadline(&daily, start, "America/Sao_Paulo"),
			Deadline {
				due: Some(date("2019-02-16")),
				due_from: instant("2019-02-16T02:00:00Z"),
				overdue_from: instant("2019-02-17T03:00:00Z"),
				provisional: false,
			}
		);
	}
}

#[test]
fn an_enormous_grace_period_is_out_of_range() {
	assert_eq!(
		schedule(u32::MAX, None)
			.deadline(instant("2026-11-07T00:00:00Z"), &TimeZone::utc()),
		Err(ScheduleError::OutOfRange),
	);
}
