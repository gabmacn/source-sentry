//! The context, the time zone, and each scope's settings, rules and schedule.

use std::collections::{BTreeMap, BTreeSet};

use chrono::NaiveDate;

use super::model::AppliedContext;
use crate::claims::{Claim, Diagnostic, DiagnosticKind, Location, Mode};
use crate::config::{
	ConfigSet, DirectorySettings, DocumentDefaults, Origin, RemoteSettings,
	Resolved, Series,
};
use crate::evaluate::schedule::{EffectiveSchedule, ScheduleError, TimeZone};
use crate::evaluate::settings::{
	DeclarationSite, Effective, EffectiveSettings, SettingOrigin,
};
use crate::model::{ClassName, Context, DocumentId, RepoPath, Rule};
use crate::policy::{PolicyError, select_context, validate_context};
use crate::{CONFIG_NAME, DIR_NAME};

/// The repository config's path, which diagnostics about the context and the
/// time zone point at.
pub(super) fn repository_config() -> RepoPath {
	#[expect(
		clippy::expect_used,
		reason = "`.source-sentry/source-sentry.yaml` is a constant, canonical \
		          repository path"
	)]
	RepoPath::new(&format!("{DIR_NAME}/{CONFIG_NAME}"))
		.expect("the repository config's path is valid")
}

/// Where a config origin points, at `field`.
pub(super) fn config_location(origin: &Origin, field: &str) -> Location {
	Location::field(origin.file.clone().unwrap_or_else(repository_config), field)
		.at(
			origin.position.map(|position| position.line),
			origin.position.map(|position| position.column),
		)
}

/// Selects the context, and validates every declared one. Every
/// problem is a configuration error. Returns `None` when no context can be
/// selected.
pub(super) fn context(
	configs: &ConfigSet,
	requested: Option<&str>,
	diagnostics: &mut Vec<Diagnostic>,
) -> Option<AppliedContext> {
	let declared_classes: BTreeSet<ClassName> =
		configs.classes().keys().cloned().collect();
	for (name, definition) in configs.contexts() {
		if let Err(error) =
			validate_context(name, &definition.context, &declared_classes)
		{
			// The first broken rule's own line, when the config recorded it.
			let rule = match &error {
				PolicyError::InvalidContext { errors, .. } => {
					errors.first().and_then(|first| {
						Some((definition.rule_origins.get(first.index)?, first.index))
					})
				}
				_ => None,
			};
			let location = match rule {
				Some((origin, index)) => config_location(
					origin,
					&format!("define.contexts.{name}.rules[{index}]"),
				),
				None => config_location(
					&definition.origin,
					&format!("define.contexts.{name}"),
				),
			};
			diagnostics.push(policy_diagnostic(&error, location));
		}
	}
	let declared: BTreeMap<String, Context> = configs
		.contexts()
		.iter()
		.map(|(name, definition)| (name.clone(), definition.context.clone()))
		.collect();
	let version = configs.version();
	match select_context(requested, &declared, version.value) {
		Ok(selected) => {
			let declared = configs.contexts().get(&selected.name);
			Some(AppliedContext {
				selected,
				definition: declared.map(|definition| definition.origin.clone()),
				rule_definitions: declared
					.map(|definition| definition.rule_origins.clone())
					.unwrap_or_default(),
			})
		}
		Err(error) => {
			let location = match &error {
				PolicyError::UnknownContext { .. } => {
					Location::field(repository_config(), "define.contexts")
				}
				PolicyError::UnsupportedDefaultsVersion { .. }
				| PolicyError::BrokenDefaults { .. }
				| PolicyError::InvalidContext { .. } => {
					config_location(&version.origin, "version")
				}
			};
			diagnostics.push(policy_diagnostic(&error, location));
			None
		}
	}
}

/// The diagnostic for a policy error at `location`.
fn policy_diagnostic(error: &PolicyError, location: Location) -> Diagnostic {
	let kind = match error {
		PolicyError::UnknownContext { .. } => DiagnosticKind::UnknownContext,
		PolicyError::InvalidContext { .. } => DiagnosticKind::InvalidContext,
		PolicyError::UnsupportedDefaultsVersion { .. } => {
			DiagnosticKind::UnsupportedDefaultsVersion
		}
		PolicyError::BrokenDefaults { .. } => DiagnosticKind::BrokenDefaults,
	};
	Diagnostic::new(kind, location, error.to_string())
}

/// The configured time zone, or `None` when it is not an IANA name, which is
/// a configuration error.
pub(super) fn time_zone(
	configs: &ConfigSet,
	diagnostics: &mut Vec<Diagnostic>,
) -> Option<TimeZone> {
	let configured = configs.time_zone();
	match TimeZone::new(&configured.value) {
		Ok(zone) => Some(zone),
		Err(error) => {
			diagnostics.push(Diagnostic::new(
				DiagnosticKind::InvalidTimeZone,
				config_location(&configured.origin, "time-zone"),
				format!(
					"{error}. Until it is corrected, no deadline or cadence window can \
					 be computed"
				),
			));
			None
		}
	}
}

/// A schedule's identity, for the cache: its grace and its cadence.
type ScheduleKey = (u32, Option<(NaiveDate, String)>);

/// Each scope's settings and rules, and each distinct schedule's validation,
/// computed once per evaluation.
pub(super) struct Scopes<'c> {
	configs: &'c ConfigSet,
	/// The applied context's name, whose scoped rules are collected.
	context: Option<String>,
	directories: BTreeMap<RepoPath, DirectorySettings>,
	remotes: BTreeMap<String, RemoteSettings>,
	schedules: BTreeMap<ScheduleKey, Result<EffectiveSchedule, ScheduleError>>,
}

impl<'c> Scopes<'c> {
	pub(super) fn new(configs: &'c ConfigSet, context: Option<String>) -> Self {
		Self {
			configs,
			context,
			directories: BTreeMap::new(),
			remotes: BTreeMap::new(),
			schedules: BTreeMap::new(),
		}
	}

	/// The settings of `directory`.
	pub(super) fn directory(
		&mut self,
		directory: &RepoPath,
	) -> &DirectorySettings {
		let configs = self.configs;
		self
			.directories
			.entry(directory.clone())
			.or_insert_with_key(|directory| configs.settings(directory))
	}

	/// The defaults and the scoped rules of the directory that holds `file`.
	pub(super) fn of_file(
		&mut self,
		file: &RepoPath,
	) -> (DocumentDefaults, Vec<Resolved<Rule>>) {
		self.of_directory(&file.parent().unwrap_or(RepoPath::ROOT))
	}

	/// The defaults and the scoped rules of `directory`.
	pub(super) fn of_directory(
		&mut self,
		directory: &RepoPath,
	) -> (DocumentDefaults, Vec<Resolved<Rule>>) {
		let context = self.context.clone();
		let settings = self.directory(directory);
		(
			settings.defaults.clone(),
			scoped(&settings.rules, context.as_deref()),
		)
	}

	/// A series' defaults and rules with its own schedule applied field by
	/// field. The final value says whether the series sets its own cadence.
	pub(super) fn of_series(
		&mut self,
		series: &Series,
	) -> (EffectiveSettings, Vec<Resolved<Rule>>, bool) {
		let (defaults, rules) = self.of_directory(&series.anchor);
		let mut settings = EffectiveSettings::new(&defaults, None);
		let Some(schedule) = &series.schedule else {
			return (settings, rules, false);
		};
		if let Some(grace) = schedule.grace {
			settings.grace = Effective {
				value: grace,
				origin: SettingOrigin::Config(series.origin.clone()),
			};
		}
		let own = !schedule.cadence.is_inherit();
		if own {
			let mut origin = series.origin.clone();
			origin.position = self
				.configs
				.series_cadence_position(series)
				.or(origin.position);
			settings.cadence = Effective {
				value: schedule.cadence.as_set().cloned(),
				origin: SettingOrigin::Config(origin),
			};
		}
		(settings, rules, own)
	}

	/// The effective settings and scoped rules of `claim`'s document: its
	/// declaration over the defaults of its directory, or of its remote owner.
	pub(super) fn of_claim(
		&mut self,
		claim: &Claim,
	) -> (EffectiveSettings, Vec<Resolved<Rule>>) {
		let (defaults, rules) = match (&claim.document, &claim.mode) {
			(DocumentId::Remote(_), Mode::RemoteLockfile { owner }) => {
				let configs = self.configs;
				let settings = self
					.remotes
					.entry(owner.clone())
					.or_insert_with_key(|owner| configs.remote_settings(owner));
				(
					settings.defaults.clone(),
					scoped(&settings.rules, self.context.as_deref()),
				)
			}
			(DocumentId::Local(path), _) => self.of_file(path),
			// A remote identity is only ever claimed by a remote lockfile, so
			// this arm is never taken; the repository's defaults are the
			// nearest meaningful ones.
			(DocumentId::Remote(_), Mode::OnFile | Mode::LocalLockfile) => {
				self.of_directory(&RepoPath::ROOT)
			}
		};
		let site = DeclarationSite {
			file: &claim.declared_in,
			field: &claim.field,
		};
		let settings =
			EffectiveSettings::new(&defaults, Some((&claim.declaration, site)));
		(settings, rules)
	}

	/// The effective schedule of `settings`, or `None` when its cadence is
	/// invalid. Config loading and discovery have already reported every
	/// invalid written cadence at its own location, so resolving a schedule
	/// does not report it again for each document or series that uses it.
	pub(super) fn schedule(
		&mut self,
		settings: &EffectiveSettings,
	) -> Option<EffectiveSchedule> {
		let key: ScheduleKey = (
			settings.grace.value,
			settings
				.cadence
				.value
				.as_ref()
				.map(|cadence| (cadence.dtstart, cadence.rrule.clone())),
		);
		self
			.schedules
			.entry(key)
			.or_insert_with(|| EffectiveSchedule::new(&settings.schedule()))
			.as_ref()
			.ok()
			.cloned()
	}
}

/// The rules that `rules` add to `context`, or none without a context.
fn scoped(
	rules: &BTreeMap<String, Vec<Resolved<Rule>>>,
	context: Option<&str>,
) -> Vec<Resolved<Rule>> {
	context
		.and_then(|context| rules.get(context))
		.cloned()
		.unwrap_or_default()
}

/// The diagnostic for a schedule error, at the origin of the cadence that
/// caused it. A config's cadence is written under `cadence_field`, such as
/// `schedule.cadence`. A declaration's claim supplies its written line.
pub(super) fn schedule_diagnostic(
	error: &ScheduleError,
	origin: &SettingOrigin,
	cadence_field: &str,
	claim: Option<&Claim>,
) -> Diagnostic {
	let (location, about) = match origin {
		SettingOrigin::Declaration { file, field } => {
			let location = claim.map_or_else(
				|| Location::field(file.clone(), field.clone()),
				|claim| claim.location_of(field.clone()),
			);
			(location, claim.map(|claim| &claim.document))
		}
		SettingOrigin::Config(origin) => {
			(config_location(origin, cadence_field), None)
		}
		SettingOrigin::BuiltIn => {
			(Location::field(repository_config(), cadence_field), None)
		}
	};
	let diagnostic = Diagnostic::schedule(error, location);
	match about {
		Some(document) => diagnostic.about(document.clone()),
		None => diagnostic,
	}
}
