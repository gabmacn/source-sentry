//! `track`: register a document as unattested, or move documents between
//! placements with `--from` and `--to`.

use std::fmt::Write as _;

use chrono::NaiveDate;
use source_sentry_core::model::{
	Cadence, CadenceSetting, ClassName, Declaration, DocumentId, Schedule,
	SourceDecl,
};
use source_sentry_core::mutate::{
	CarriedTombstone, PatternMatches, Placement, Registered, Setting, Target,
	TrackFromRequest, TrackFromSummary, TrackRequest, TrackSummary, track,
	track_from,
};
use source_sentry_core::report;

use crate::cli::{DocumentArg, PlacementArg, TrackArgs};
use crate::describe;
use crate::exit::Failure;
use crate::output::{self, Printer};
use crate::workspace::{Workspace, load};

/// Runs `track` on the working tree.
///
/// # Errors
///
/// Exit 64 for a combination of options that `docs/cli.md` does not allow,
/// and the refusals of the planner and of confirmation; see
/// [`crate::exit`].
pub(super) fn run(
	workspace: &Workspace,
	args: &TrackArgs,
	printer: Printer,
) -> Result<u8, Failure> {
	let snapshot = workspace
		.repository
		.working_tree_snapshot()
		.map_err(|error| Failure::backend(&error))?;
	let (configs, inventory) = load(&snapshot)?;
	let (plan, summary) = if let (Some(from), Some(to)) = (&args.from, &args.to) {
		let request = TrackFromRequest {
			documents: args
				.documents
				.iter()
				.map(|document| workspace.document(document))
				.collect::<Result<_, _>>()?,
			from: placement(workspace, from)?,
			to: target(workspace, to)?,
		};
		let planned = track_from(&snapshot, &configs, &inventory, &request)
			.map_err(|error| Failure::mutate(&error))?;
		(planned.plan, moved_summary(&planned.summary))
	} else {
		let request = register_request(workspace, args)?;
		let planned = track(&snapshot, &configs, &inventory, &request)
			.map_err(|error| Failure::mutate(&error))?;
		(planned.plan, registered_summary(&planned.summary))
	};
	super::apply::confirm_and_apply(
		workspace,
		&plan,
		&summary,
		args.confirm,
		printer,
	)
}

/// The request to register one document.
fn register_request(
	workspace: &Workspace,
	args: &TrackArgs,
) -> Result<TrackRequest, Failure> {
	let [document] = args.documents.as_slice() else {
		return Err(Failure::usage(
			"`track` registers one document; to move several, give `--from` and \
			 `--to`",
		));
	};
	let target = if args.on_file {
		Target::OnFile
	} else if let Some(directory) = &args.lockfile {
		Target::Lockfile {
			directory: directory
				.as_deref()
				.map(|directory| workspace.repo_path(directory))
				.transpose()?,
		}
	} else if let Some(owner) = &args.remote {
		Target::Remote {
			owner: owner.clone(),
		}
	} else if let DocumentArg::Remote(identity) = document {
		return Err(Failure::usage(format!(
			"`{identity}` is a remote document: name its owner with --remote <owner>"
		)));
	} else {
		// The nearest lockfile, or a new one in the document's directory.
		Target::Lockfile { directory: None }
	};
	Ok(TrackRequest {
		document: workspace.document(document)?,
		target,
		placement_named: args.on_file
			|| args.lockfile.is_some()
			|| args.remote.is_some(),
		sources: args.ordered.clone(),
		classes: args.classes.clone(),
		owners: args.owners.clone(),
		schedule: schedule(args)?,
		url: args.url.clone(),
	})
}

/// The schedule that `--grace`, `--cadence` and `--no-cadence` write, if
/// any.
fn schedule(args: &TrackArgs) -> Result<Option<Schedule>, Failure> {
	let cadence = if args.no_cadence {
		CadenceSetting::Remove
	} else if let Some(values) = &args.cadence {
		let [dtstart, rrule] = values.as_slice() else {
			return Err(Failure::usage(
				"--cadence takes two values: <dtstart> <rrule>",
			));
		};
		let dtstart =
			NaiveDate::parse_from_str(dtstart, "%Y-%m-%d").map_err(|_| {
				Failure::usage(format!(
					"the cadence's start `{dtstart}` is not a date such as 2026-12-01"
				))
			})?;
		CadenceSetting::Set(Cadence {
			dtstart,
			rrule: rrule.clone(),
		})
	} else {
		CadenceSetting::Inherit
	};
	if args.grace.is_none() && cadence.is_inherit() {
		return Ok(None);
	}
	Ok(Some(Schedule {
		cadence,
		grace: args.grace,
	}))
}

/// A placement, with its directory converted to a repository path.
fn placement(
	workspace: &Workspace,
	placement: &PlacementArg,
) -> Result<Placement, Failure> {
	Ok(match placement {
		PlacementArg::OnFile => Placement::OnFile,
		PlacementArg::Lockfile(directory) => Placement::Lockfile {
			directory: workspace.repo_path(directory)?,
		},
		PlacementArg::Remote(owner) => Placement::Remote {
			owner: owner.clone(),
		},
	})
}

/// Where `--to` moves documents.
fn target(
	workspace: &Workspace,
	placement: &PlacementArg,
) -> Result<Target, Failure> {
	Ok(match placement {
		PlacementArg::OnFile => Target::OnFile,
		PlacementArg::Lockfile(directory) => Target::Lockfile {
			directory: Some(workspace.repo_path(directory)?),
		},
		PlacementArg::Remote(owner) => Target::Remote {
			owner: owner.clone(),
		},
	})
}

/// What registering records.
fn registered_summary(summary: &TrackSummary) -> String {
	let mut text = String::new();
	registration(&mut text, &summary.registered);
	carried(&mut text, &summary.carried);
	text
}

/// One registration, as `track` and `init` print it.
pub(super) fn registration(text: &mut String, registered: &Registered) {
	let created = if registered.created {
		", a new lockfile whose directory becomes a root"
	} else {
		""
	};
	if registered.extended {
		let _ = writeln!(
			text,
			"Add to the declaration of {}, {} ({}); new sources are unattested",
			output::bold(&registered.document.to_string()),
			placement_name(&registered.placement),
			describe::location(&registered.declared_in)
		);
	} else {
		let _ = writeln!(
			text,
			"Track {} as unattested, {} ({}{created})",
			output::bold(&registered.document.to_string()),
			placement_name(&registered.placement),
			describe::location(&registered.declared_in)
		);
	}
	if let Some(inherited) = &registered.inherited {
		let _ = writeln!(
			text,
			"  Inherit from {} without reviews, seal or completion:",
			inherited.predecessor,
		);
		declaration_fields(text, &inherited.declaration, &registered.matches);
		text.push_str("  Result after command options:\n");
	}
	declaration_fields(text, &registered.declaration, &registered.matches);
}

/// Declared sources and explicit settings, shared by inherited and final values.
fn declaration_fields(
	text: &mut String,
	declaration: &Declaration,
	patterns: &[PatternMatches],
) {
	for source in &declaration.sources {
		match source {
			SourceDecl::Path(path) => {
				let _ = writeln!(text, "  source   {}", path.path);
			}
			SourceDecl::Pattern(pattern) => {
				let matches = patterns
					.iter()
					.find(|found| found.pattern == pattern.pattern)
					.map_or(0, |found| found.matches.len());
				let _ = writeln!(
					text,
					"  pattern  {} ({})",
					pattern.pattern,
					describe::count(matches, "match", "matches")
				);
			}
		}
	}
	if !declaration.classes.is_empty() {
		let classes: Vec<&str> =
			declaration.classes.iter().map(ClassName::as_str).collect();
		let _ = writeln!(text, "  classes  {}", classes.join(", "));
	}
	if !declaration.owners.is_empty() {
		let _ = writeln!(text, "  owners   {}", declaration.owners.join(", "));
	}
	if let Some(schedule) = &declaration.schedule {
		if let Some(grace) = schedule.grace {
			let _ = writeln!(
				text,
				"  grace    {}",
				describe::count(grace as usize, "day", "days")
			);
		}
		match &schedule.cadence {
			CadenceSetting::Inherit => {}
			CadenceSetting::Remove => {
				text.push_str("  cadence  none, even if one is inherited\n");
			}
			CadenceSetting::Set(cadence) => {
				let _ = writeln!(
					text,
					"  cadence  {} from {}",
					cadence.rrule, cadence.dtstart
				);
			}
		}
	}
	if let Some(url) = &declaration.url {
		let _ = writeln!(text, "  url      {url}");
	}
}

/// Where a placement is, briefly.
pub(super) fn placement_name(placement: &Placement) -> String {
	match placement {
		Placement::OnFile => "on-file".to_owned(),
		Placement::Lockfile { directory } => {
			format!("lockfile:{}", report::directory(directory))
		}
		Placement::Remote { owner } => format!("remote:{owner}"),
	}
}

/// Tombstones that move with a lockfile change.
pub(super) fn carried(text: &mut String, carried: &[CarriedTombstone]) {
	for tombstone in carried {
		let _ = writeln!(
			text,
			"  the tombstones of {} move from {} to {}",
			tombstone.document, tombstone.from, tombstone.to
		);
	}
}

/// What a move changes, with each document's settings before and after.
fn moved_summary(summary: &TrackFromSummary) -> String {
	let mut text = String::new();
	for moved in &summary.moved {
		let created = if moved.created {
			", a new lockfile"
		} else {
			""
		};
		let _ = writeln!(
			text,
			"Move {} from {} to {}{created}",
			output::bold(&document_name(&moved.document)),
			describe::location(&moved.from),
			describe::location(&moved.to)
		);
		let preview = &moved.settings;
		if preview.changed.is_empty() {
			text.push_str("  its effective settings stay the same\n");
		}
		for setting in &preview.changed {
			let (name, before, after) = match setting {
				Setting::Classes => (
					"classes",
					class_names(&preview.before.classes.value),
					class_names(&preview.after.classes.value),
				),
				Setting::Owners => (
					"owners",
					names(&preview.before.owners.value),
					names(&preview.after.owners.value),
				),
				Setting::Grace => (
					"grace",
					describe::count(preview.before.grace.value as usize, "day", "days"),
					describe::count(preview.after.grace.value as usize, "day", "days"),
				),
				Setting::Cadence => (
					"cadence",
					cadence(preview.before.cadence.value.as_ref()),
					cadence(preview.after.cadence.value.as_ref()),
				),
			};
			let _ = writeln!(text, "  {name} changes: {before} → {after}");
		}
	}
	carried(&mut text, &summary.carried);
	text
}

fn document_name(document: &DocumentId) -> String {
	document.to_string()
}

/// Class names separated by commas, or `none`.
fn class_names(classes: &[ClassName]) -> String {
	names(&classes.iter().map(ClassName::as_str).collect::<Vec<_>>())
}

/// Names separated by commas, or `none`.
fn names<T: AsRef<str>>(names: &[T]) -> String {
	if names.is_empty() {
		"none".to_owned()
	} else {
		names
			.iter()
			.map(AsRef::as_ref)
			.collect::<Vec<_>>()
			.join(", ")
	}
}

/// A cadence, or `none`.
fn cadence(cadence: Option<&Cadence>) -> String {
	cadence.map_or_else(
		|| "none".to_owned(),
		|cadence| format!("{} from {}", cadence.rrule, cadence.dtstart),
	)
}
