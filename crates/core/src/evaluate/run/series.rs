//! Series: living heads, `awaiting-seal`, and cadence with its completions.

use std::collections::BTreeSet;

use chrono::{DateTime, Days, Utc};

use super::clocks::{Answer, Answers, Requests, Slot, clock_diagnostic};
use super::documents::{add_former_sites, local_lockfile_site, sites};
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
use crate::evaluate::settings::EffectiveSettings;
use crate::model::{DocumentId, Fact, Lifecycle, RepoPath, Review, Rule, Seal};

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
	let (settings, rules, own) = scopes.of_series(series);
	let cadence_field = if own {
		format!("series.{}.schedule.cadence", series.name)
	} else {
		"schedule.cadence".to_owned()
	};
	let has_cadence = settings.cadence.value.is_some();
	let schedule = if has_cadence {
		scopes.schedule(&settings)
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
			for (query, location) in
				completion_queries(series.lifecycle, &member.path, inventory, scopes)
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

/// The questions dating every completion of `member`, including those of
/// earlier documents at the same path. Each marker is matched separately in
/// history, so a tombstone credits the window of its own committed marker.
fn completion_queries(
	lifecycle: Lifecycle,
	member: &RepoPath,
	inventory: &Inventory,
	scopes: &mut Scopes<'_>,
) -> Vec<(Query, Location)> {
	let document = DocumentId::Local(member.clone());
	let mut queries = Vec::new();
	for tombstone in inventory
		.tombstones
		.iter()
		.filter(|tombstone| tombstone.document == document)
	{
		if let Some((marker, condition)) = completion_condition(
			lifecycle,
			tombstone.retired.seal.as_ref(),
			tombstone.retired.completed.as_ref(),
			&queries,
		) {
			let mut sites = vec![local_lockfile_site(
				crate::config::project_of(&inventory.projects, &tombstone.lockfile),
				tombstone.lockfile.clone(),
			)];
			add_former_sites(&document, inventory, &mut sites);
			queries.push((
				Query {
					project: inventory.project.clone(),
					projects: inventory.projects.clone(),
					renamed: Vec::new(),
					document: document.clone(),
					sites,
					condition,
				},
				tombstone.location_of(format!("{}.{marker}", tombstone.field)),
			));
		}
	}
	for claim in inventory.claims_of(&document) {
		if let Some((marker, condition)) = completion_condition(
			lifecycle,
			claim.declaration.seal.as_ref(),
			claim.declaration.completed.as_ref(),
			&queries,
		) {
			queries.push((
				Query {
					project: inventory.project.clone(),
					projects: inventory.projects.clone(),
					renamed: claim.declaration.renamed.clone(),
					document: document.clone(),
					sites: sites(claim, inventory, scopes),
					condition,
				},
				claim.location_of(format!("{}.{marker}", claim.field)),
			));
		}
	}

	queries
}

/// The marker used by `lifecycle` and its occurrence among earlier queries.
/// Counting identical markers distinguishes repeated completions even when
/// the recorded instant, author and record content all match.
fn completion_condition(
	lifecycle: Lifecycle,
	seal: Option<&Seal>,
	completed: Option<&Review>,
	previous: &[(Query, Location)],
) -> Option<(&'static str, Condition)> {
	match lifecycle {
		Lifecycle::RecordsOnly => seal.map(|seal| {
			let occurrence = previous
				.iter()
				.filter(|(query, _)| {
					matches!(
						&query.condition,
						Condition::Sealed { seal: earlier, .. } if earlier == seal
					)
				})
				.count()
				+ 1;
			(
				"seal",
				Condition::Sealed {
					seal: seal.clone(),
					occurrence,
				},
			)
		}),
		Lifecycle::Living => completed.map(|completed| {
			let occurrence = previous
				.iter()
				.filter(|(query, _)| {
					matches!(
						&query.condition,
						Condition::Completed { completed: earlier, .. }
							if earlier == completed
					)
				})
				.count()
				+ 1;
			(
				"completed",
				Condition::Completed {
					completed: completed.clone(),
					occurrence,
				},
			)
		}),
	}
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
	let completed: Vec<Completion<(RepoPath, usize)>> = completions
		.iter()
		.enumerate()
		.filter_map(|(index, completion)| match &completion.state {
			CompletionState::Completed(start) => Some(Completion {
				entry: (completion.member.clone(), index),
				at: start.timestamp,
			}),
			CompletionState::Uncommitted | CompletionState::Failed => None,
		})
		.collect();
	// Cadence deduplicates entry identities. Each completed incarnation of a
	// reused path is a separate entry; keep its index until credits are assigned.
	let status = cadence::evaluate(recurrence, grace, zone, &completed, at)?;
	let status = CadenceStatus {
		windows: status
			.windows
			.into_iter()
			.map(|window| cadence::Window {
				after: window.after,
				due: window.due,
				last_day: window.last_day,
				closed: window.closed,
				credit: window.credit.map(|credit| Completion {
					entry: credit.entry.0,
					at: credit.at,
				}),
			})
			.collect(),
		missed: status.missed,
		overdue: status.overdue,
	};
	let next = next_deadline(&status, recurrence, grace, zone)?;
	Ok((status, next))
}

/// The deadline of the first window that has no credit and is not missed:
/// the window that makes the series overdue, or the next one due. When every
/// window opened so far is credited or missed, it is the deadline of the
/// window after the last one, which the cadence has not opened yet.
fn next_deadline(
	status: &CadenceStatus<RepoPath>,
	recurrence: &Recurrence,
	grace: u32,
	zone: &TimeZone,
) -> Result<Option<Deadline>, ScheduleError> {
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
		(None, Some(last)) => {
			let after_due = last
				.due
				.checked_add_days(Days::new(1))
				.ok_or(ScheduleError::OutOfRange)?;
			recurrence
				.next_on_or_after(after_due)?
				.checked_add_days(Days::new(u64::from(grace)))
				.ok_or(ScheduleError::OutOfRange)?
		}
		(None, None) => return Ok(None),
	};
	let after = last_day
		.checked_add_days(Days::new(1))
		.ok_or(ScheduleError::OutOfRange)?;
	Ok(Some(Deadline {
		due: Some(last_day),
		overdue_from: zone.start_of_day(after)?,
	}))
}
