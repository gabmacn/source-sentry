//! Schedules resolved for an evaluation: what documents on one cadence share,
//! and what each keeps for itself.

use jiff::civil::Date;

use super::{REPOSITORY, Repo};
use crate::config::ConfigSet;
use crate::evaluate::run::scopes::Scopes;
use crate::evaluate::schedule::{EffectiveSchedule, TimeZone};
use crate::evaluate::settings::EffectiveSettings;
use crate::model::RepoPath;

/// A repository config whose cadence falls on Mondays, from 5 January 2026.
const WEEKLY: &str = "version: 1
schedule:
  cadence:
    dtstart: 2026-01-05
    rrule: 'FREQ=WEEKLY'
";

/// The repository's settings with each of `graces`, and their schedules as
/// one evaluation resolves them, in that order.
fn schedules(
	scopes: &mut Scopes<'_>,
	graces: &[u32],
) -> Vec<(EffectiveSettings, EffectiveSchedule)> {
	let (defaults, _) = scopes.of_directory(&RepoPath::ROOT);
	let base = EffectiveSettings::new(&defaults, None);
	graces
		.iter()
		.map(|&grace| {
			let mut settings = base.clone();
			settings.grace.value = grace;
			let schedule = scopes.schedule(&settings).unwrap();
			(settings, schedule)
		})
		.collect()
}

/// The repository's configs.
fn configs(config: &str) -> ConfigSet {
	let repo = Repo::new(&[(REPOSITORY, config)]);
	ConfigSet::load_project(&repo.checkout(), &RepoPath::ROOT).unwrap()
}

#[test]
fn schedules_on_one_cadence_share_its_expansion_whatever_their_grace() {
	let configs = configs(WEEKLY);
	let mut scopes = Scopes::new(&configs, None);
	let resolved = schedules(&mut scopes, &[0, 3, 10, 3]);
	assert_eq!(scopes.expansions(), 1);
	let graces: Vec<u32> = resolved
		.iter()
		.map(|(_, schedule)| schedule.grace)
		.collect();
	assert_eq!(graces, [0, 3, 10, 3]);

	let (defaults, _) = scopes.of_directory(&RepoPath::ROOT);
	let mut other = EffectiveSettings::new(&defaults, None);
	let cadence = other.cadence.value.as_mut().unwrap();
	cadence.rrule = "FREQ=WEEKLY;INTERVAL=2".to_owned();
	scopes.schedule(&other).unwrap();
	assert_eq!(scopes.expansions(), 2);
}

/// Each schedule on a shared cadence keeps its own grace, whichever of them
/// first expands the cadence furthest.
#[test]
fn deadlines_on_a_shared_cadence_keep_each_schedules_grace() {
	let configs = configs(WEEKLY);
	let mut scopes = Scopes::new(&configs, None);
	let zone = TimeZone::new("UTC").unwrap();
	// A Wednesday.
	let start: jiff::Timestamp = "2026-01-07T12:00:00Z".parse().unwrap();
	let expected = [
		(400, "2027-02-15"),
		(20, "2026-02-02"),
		(6, "2026-01-19"),
		(3, "2026-01-12"),
		(0, "2026-01-12"),
	];
	let graces: Vec<u32> = expected.iter().map(|&(grace, _)| grace).collect();
	for ((settings, schedule), (grace, due)) in
		schedules(&mut scopes, &graces).iter().zip(expected)
	{
		let due: Date = due.parse().unwrap();
		let deadline = schedule.deadline(start, &zone).unwrap();
		assert_eq!(deadline.due, Some(due), "grace {grace}");
		let alone = EffectiveSchedule::new(&settings.schedule()).unwrap();
		assert_eq!(*schedule, alone, "grace {grace}");
		assert_eq!(deadline, alone.deadline(start, &zone).unwrap());
	}
	assert_eq!(scopes.expansions(), 1);
}
