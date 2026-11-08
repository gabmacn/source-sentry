//! Series: living heads, `awaiting-seal`, and cadence with its completions.

use std::collections::BTreeSet;

use chrono::{DateTime, Days, Utc};

use super::clocks::{Answer, Answers, Requests, Slot, clock_diagnostic};
use super::documents::{local_lockfile_site, sites};
use super::history::{Condition, Query};
use super::model::{
	CompletionState, MemberCompletion, SeriesCadence, SeriesKey, SeriesSubject,
	Subject, SubjectKind,
};
use super::scopes::{Scopes, config_location, schedule_diagnostic};
use crate::claims::{
	Diagnostic, DiagnosticKind, Inventory, Location, MemberState, SeriesMembers,
};
use crate::config::{Resolved, Series};
use crate::evaluate::cadence::{self, CadenceStatus, Completion};
use crate::evaluate::schedule::{
	Deadline, EffectiveSchedule, Recurrence, ScheduleError, TimeZone,
};
use crate::evaluate::settings::{Effective, EffectiveSettings, SettingOrigin};
use crate::model::{DocumentId, Fact, Lifecycle, RepoPath, Rule};

/// One series, ready for its completions to be answered.
pub(super) struct SeriesPlan<'a> {
	members: &'a SeriesMembers,
	settings: EffectiveSettings,
	scoped: Vec<Resolved<Rule>>,
	/// Where the cadence is written, for diagnostics.
	cadence_field: String,
	/// The validated schedule, when the series has a valid cadence.
	schedule: Option<EffectiveSchedule>,
	/// Whether the effective schedule has a cadence, valid or not.
	has_cadence: bool,
	head: Option<RepoPath>,
	/// The members that carry `awaiting-seal` when claimed and unsealed.
	awaiting: Vec<RepoPath>,
	/// Each member that may have completed, with its pending clock and where
	/// its seal or `completed` is written.
	completions: Vec<(RepoPath, Slot, Location)>,
}

/// Plans `members`: its settings, its head and the members awaiting a seal,
/// and the completion clocks its cadence needs.
pub(super) fn plan<'a>(
	members: &'a SeriesMembers,
	inventory: &Inventory,
	scopes: &mut Scopes<'_>,
	requests: &mut Requests,
	diagnostics: &mut Vec<Diagnostic>,
) -> SeriesPlan<'a> {
	let series = &members.series;
	let (defaults, rules) = scopes.of_directory(&series.anchor);
	let mut settings = EffectiveSettings::new(&defaults, None);
	let own = override_schedule(&mut settings, series);
	let cadence_field = if own {
		format!("series.{}.schedule.cadence", series.name)
	} else {
		"schedule.cadence".to_owned()
	};
	let has_cadence = settings.cadence.value.is_some();
	let schedule = if has_cadence {
		scopes.schedule(&settings, &cadence_field, None, diagnostics)
	} else {
		None
	};

	let mut head = None;
	let awaiting: Vec<RepoPath> = match series.lifecycle {
		Lifecycle::Living => {
			if let Some(newest) = members.members.last() {
				check_head(series, newest, inventory, diagnostics);
				head = Some(newest.path.clone());
			}
			let older = members.members.len().saturating_sub(1);
			members.members[..older]
				.iter()
				.filter(|member| member.claimed)
				.map(|member| member.path.clone())
				.collect()
		}
		Lifecycle::RecordsOnly => members
			.members
			.iter()
			.filter(|member| member.claimed)
			.map(|member| member.path.clone())
			.collect(),
	};

	let mut completions = Vec::new();
	if schedule.is_some() {
		for member in &members.members {
			if let Some((query, location)) =
				completion(series.lifecycle, &member.path, inventory, scopes)
			{
				completions.push((
					member.path.clone(),
					requests.start(query),
					location,
				));
			}
		}
	}

	SeriesPlan {
		members,
		settings,
		scoped: rules,
		cadence_field,
		schedule,
		has_cadence,
		head,
		awaiting,
		completions,
	}
}

/// Applies the series' own schedule over its scope's, field by field, and
/// says whether it set the cadence.
fn override_schedule(
	settings: &mut EffectiveSettings,
	series: &Series,
) -> bool {
	let Some(schedule) = &series.schedule else {
		return false;
	};
	let origin = || SettingOrigin::Config(series.origin.clone());
	if let Some(grace) = schedule.grace {
		settings.grace = Effective {
			value: grace,
			origin: origin(),
		};
	}
	if schedule.cadence.is_inherit() {
		return false;
	}
	settings.cadence = Effective {
		value: schedule.cadence.as_set().cloned(),
		origin: origin(),
	};
	true
}

/// Reports a `living` series whose newest entry is missing or sealed.
fn check_head(
	series: &Series,
	newest: &crate::claims::SeriesMember,
	inventory: &Inventory,
	diagnostics: &mut Vec<Diagnostic>,
) {
	let location =
		config_location(&series.origin, &format!("series.{}", series.name));
	let document = DocumentId::Local(newest.path.clone());
	let related = vec![Location::file(newest.path.clone())];
	if newest.state != MemberState::Present {
		let how = match newest.state {
			MemberState::Retired => "only a tombstone names it",
			MemberState::Missing | MemberState::Present => {
				"a lockfile lists it, but the snapshot lacks it"
			}
		};
		diagnostics.push(
			Diagnostic::new(
				DiagnosticKind::MissingHead,
				location.clone(),
				format!(
					"the newest entry of the living series `{}`, `{}`, is missing: {how}. \
					 A sealed predecessor never becomes living again; restore the \
					 entry, add a new successor, or retire the series",
					series.name, newest.path
				),
			)
			.with_related(related.clone())
			.about(document.clone()),
		);
	}
	let sealed = inventory
		.claims_of(&document)
		.any(crate::claims::Claim::is_sealed)
		|| inventory.tombstones.iter().any(|tombstone| {
			tombstone.document == document && tombstone.retired.seal.is_some()
		});
	if sealed {
		diagnostics.push(
			Diagnostic::new(
				DiagnosticKind::SealedHead,
				location,
				format!(
					"the newest entry of the living series `{}`, `{}`, is sealed, but \
					 a living series' newest entry is its living document. Add a new \
					 successor, or make the series `records-only`",
					series.name, newest.path
				),
			)
			.with_related(related)
			.about(document),
		);
	}
}

/// The question that dates `member`'s completion, and where its marker is
/// written, or `None` when it has not completed in the evaluated snapshot:
///
/// - in a `records-only` series, the commit that introduced its seal, in its
///   lockfile entry or its tombstone;
/// - in a `living` series, the commit that introduced its `completed`.
fn completion(
	lifecycle: Lifecycle,
	member: &RepoPath,
	inventory: &Inventory,
	scopes: &mut Scopes<'_>,
) -> Option<(Query, Location)> {
	let document = DocumentId::Local(member.clone());
	let (marker, condition) = match lifecycle {
		Lifecycle::RecordsOnly => ("seal", Condition::Sealed),
		Lifecycle::Living => ("completed", Condition::Completed),
	};
	let claim = inventory
		.claims_of(&document)
		.find(|claim| match lifecycle {
			Lifecycle::RecordsOnly => claim.is_sealed(),
			Lifecycle::Living => claim.declaration.completed.is_some(),
		});
	if let Some(claim) = claim {
		let mut location = claim.location();
		location.field = Some(format!("{}.{marker}", claim.field));
		let query = Query {
			document,
			sites: sites(claim, inventory, scopes),
			condition,
		};
		return Some((query, location));
	}
	if lifecycle != Lifecycle::RecordsOnly {
		return None;
	}
	let tombstone = inventory.tombstones.iter().find(|tombstone| {
		tombstone.document == document && tombstone.retired.seal.is_some()
	})?;
	let location = Location::field(
		tombstone.lockfile.clone(),
		format!("{}.seal", tombstone.field),
	);
	let query = Query {
		document,
		sites: vec![local_lockfile_site(tombstone.lockfile.clone())],
		condition,
	};
	Some((query, location))
}

/// The members of `plan` that carry `awaiting-seal`, with the series that
/// makes them.
pub(super) fn awaiting<'p>(
	plan: &'p SeriesPlan<'_>,
) -> (SeriesKey, &'p [RepoPath]) {
	let series = &plan.members.series;
	(
		SeriesKey {
			scope: series.scope.clone(),
			name: series.name.clone(),
			lifecycle: series.lifecycle,
		},
		&plan.awaiting,
	)
}

/// Finishes a planned series: dates its completions and evaluates its
/// cadence, giving `overdue` and `missed`.
pub(super) fn finish(
	plan: SeriesPlan<'_>,
	answers: &Answers,
	zone: Option<&TimeZone>,
	at: DateTime<Utc>,
	diagnostics: &mut Vec<Diagnostic>,
) -> Subject {
	let SeriesPlan {
		members,
		settings,
		scoped: rules,
		cadence_field,
		schedule,
		has_cadence,
		head,
		completions,
		..
	} = plan;
	let mut facts = BTreeSet::new();
	let mut deadline = None;
	let cadence = has_cadence.then(|| {
		let completions = dated(completions, answers, diagnostics);
		let status = match (&schedule, zone) {
			(
				Some(EffectiveSchedule {
					grace,
					cadence: Some(recurrence),
				}),
				Some(zone),
			) => {
				let evaluated = windows(recurrence, *grace, zone, &completions, at);
				match evaluated {
					Ok((status, next)) => {
						deadline = next;
						Some(status)
					}
					Err(error) => {
						diagnostics.push(schedule_diagnostic(
							&error,
							&settings.cadence.origin,
							&cadence_field,
							None,
						));
						None
					}
				}
			}
			_ => None,
		};
		if let Some(status) = &status {
			if status.overdue {
				facts.insert(Fact::Overdue);
			}
			if !status.missed.is_empty() {
				facts.insert(Fact::Missed);
			}
		}
		SeriesCadence {
			completions,
			status,
		}
	});
	Subject {
		kind: SubjectKind::Series(SeriesSubject {
			series: members.series.clone(),
			members: members.members.clone(),
			head,
			cadence,
		}),
		settings,
		facts,
		deadline,
		scoped_rules: rules,
		judgement: None,
	}
}

/// Each pending completion, dated. Uncommitted markers do not count yet,
/// and a completion that cannot be dated is reported.
fn dated(
	pending: Vec<(RepoPath, Slot, Location)>,
	answers: &Answers,
	diagnostics: &mut Vec<Diagnostic>,
) -> Vec<MemberCompletion> {
	pending
		.into_iter()
		.map(|(member, slot, location)| {
			let state = match answers.get(slot) {
				Answer::Started(start) => CompletionState::Completed(start.clone()),
				Answer::Fresh | Answer::NotStarted | Answer::Unknown => {
					CompletionState::Uncommitted
				}
				Answer::Failed(error) => {
					let document = DocumentId::Local(member.clone());
					let what = format!("the completion of `{member}`");
					diagnostics.push(clock_diagnostic(error, location, &document, &what));
					CompletionState::Failed
				}
			};
			MemberCompletion { member, state }
		})
		.collect()
}

/// The cadence's windows and credits at `at`, and the next deadline.
///
/// # Errors
///
/// Fails when the cadence has no occurrence where one is needed, or a date
/// leaves the calendar.
fn windows(
	recurrence: &Recurrence,
	grace: u32,
	zone: &TimeZone,
	completions: &[MemberCompletion],
	at: DateTime<Utc>,
) -> Result<(CadenceStatus<RepoPath>, Option<Deadline>), ScheduleError> {
	let completed: Vec<Completion<RepoPath>> = completions
		.iter()
		.filter_map(|completion| match &completion.state {
			CompletionState::Completed(start) => Some(Completion {
				entry: completion.member.clone(),
				at: start.timestamp,
			}),
			CompletionState::Uncommitted | CompletionState::Failed => None,
		})
		.collect();
	let status = cadence::evaluate(recurrence, grace, zone, &completed, at)?;
	let next = next_deadline(&status, zone)?;
	Ok((status, next))
}

/// The deadline of the first window that has no credit and is not missed:
/// the window that makes the series overdue, or the next one due.
fn next_deadline(
	status: &CadenceStatus<RepoPath>,
	zone: &TimeZone,
) -> Result<Option<Deadline>, ScheduleError> {
	let Some(window) = status
		.windows
		.iter()
		.enumerate()
		.find(|(index, window)| {
			window.credit.is_none() && !status.missed.contains(index)
		})
		.map(|(_, window)| window)
	else {
		return Ok(None);
	};
	let after = window
		.last_day
		.checked_add_days(Days::new(1))
		.ok_or(ScheduleError::OutOfRange)?;
	Ok(Some(Deadline {
		due: Some(window.last_day),
		overdue_from: zone.start_of_day(after)?,
	}))
}
