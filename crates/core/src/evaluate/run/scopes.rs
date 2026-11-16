//! The context, the time zone, and each scope's settings, rules and schedule.

use std::collections::{BTreeMap, BTreeSet};

use chrono::NaiveDate;

use super::model::AppliedContext;
use crate::claims::{Claim, Diagnostic, DiagnosticKind, Location, Mode};
use crate::config::{
	ConfigSet, ContextDefinition, DirectorySettings, DocumentDefaults, Origin,
	RemoteSettings, Resolved, Series,
};
use crate::evaluate::schedule::{
	EffectiveSchedule, Recurrence, ScheduleError, TimeZone,
};
use crate::evaluate::settings::{
	DeclarationSite, Effective, EffectiveSettings, SettingOrigin,
};
use crate::model::{ClassName, Context, DocumentId, RepoPath, Rule};
use crate::policy::{PolicyError, select_context, validate_context};
use crate::{CONFIG_NAME, DIR_NAME};

/// The repository config's path, where [`config_location`] and a built-in
/// schedule locate a value that has no config file of its own.
pub(super) fn repository_config() -> RepoPath {
	#[expect(
		clippy::expect_used,
		reason = "`.source-sentry/source-sentry.yaml` is a constant, canonical \
		          repository path"
	)]
	RepoPath::new(&format!("{DIR_NAME}/{CONFIG_NAME}"))
		.expect("the repository config's path is valid")
}

/// The path of the config at the root of `configs`' project, which
/// diagnostics about the project's contexts and settings point at.
fn project_config(configs: &ConfigSet) -> RepoPath {
	#[expect(
		clippy::expect_used,
		reason = "a project root is a canonical repository path, and the config \
		          name below it is a constant, canonical relative path"
	)]
	configs
		.project()
		.join(&format!("{DIR_NAME}/{CONFIG_NAME}"))
		.expect("a project's config path is valid")
}

/// Where a config origin points, at `field`, for an origin that always has
/// a file of its own, such as a series'. A value without one would be
/// located at the repository config; [`project_location`] locates it at the
/// evaluated project's config instead.
pub(super) fn config_location(origin: &Origin, field: &str) -> Location {
	located(origin, field, repository_config)
}

/// Where a config origin of `configs`' project points, at `field`. A value
/// with no file of its own, such as the built-in time zone, is located at
/// the project's own config, never at an enclosing repository's.
pub(super) fn project_location(
	configs: &ConfigSet,
	origin: &Origin,
	field: &str,
) -> Location {
	located(origin, field, || project_config(configs))
}

/// Where `origin` points, at `field`, or at the config `fallback` names when
/// the value has no file of its own.
fn located(
	origin: &Origin,
	field: &str,
	fallback: impl FnOnce() -> RepoPath,
) -> Location {
	Location::field(origin.file.clone().unwrap_or_else(fallback), field).at(
		origin.position.map(|position| position.line),
		origin.position.map(|position| position.column),
	)
}

/// Selects the context, and validates every declared one, and every one
/// that a preset defines without being imported. Every problem is a
/// configuration error. Returns `None` when no context can be selected.
pub(super) fn context(
	configs: &ConfigSet,
	requested: Option<&str>,
	diagnostics: &mut Vec<Diagnostic>,
) -> Option<AppliedContext> {
	let declared_classes: BTreeSet<ClassName> =
		configs.classes().keys().cloned().collect();
	for (name, definition) in configs.contexts() {
		validate_definition(
			configs,
			name,
			definition,
			&declared_classes,
			diagnostics,
		);
	}
	for unimported in configs.unimported_definitions() {
		let declared: BTreeSet<ClassName> = declared_classes
			.iter()
			.chain(&unimported.classes)
			.cloned()
			.collect();
		for (name, definition) in &unimported.contexts {
			validate_definition(configs, name, definition, &declared, diagnostics);
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
					Location::field(project_config(configs), "define.contexts")
				}
				PolicyError::UnsupportedDefaultsVersion { .. }
				| PolicyError::BrokenDefaults { .. }
				| PolicyError::InvalidContext { .. } => {
					project_location(configs, &version.origin, "version")
				}
			};
			diagnostics.push(policy_diagnostic(&error, location));
			None
		}
	}
}

/// Validates one defined context of `configs`' project against the
/// `declared` classes, and reports what is wrong with it at its first broken
/// rule.
fn validate_definition(
	configs: &ConfigSet,
	name: &str,
	definition: &ContextDefinition,
	declared: &BTreeSet<ClassName>,
	diagnostics: &mut Vec<Diagnostic>,
) {
	let Err(error) = validate_context(name, &definition.context, declared) else {
		return;
	};
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
		Some((origin, index)) => project_location(
			configs,
			origin,
			&format!("define.contexts.{name}.rules[{index}]"),
		),
		None => project_location(
			configs,
			&definition.origin,
			&format!("define.contexts.{name}"),
		),
	};
	diagnostics.push(policy_diagnostic(&error, location));
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
				project_location(configs, &configured.origin, "time-zone"),
				format!(
					"{error}. Until it is corrected, no deadline or cadence window can \
					 be computed"
				),
			));
			None
		}
	}
}

/// A cadence's identity, for the cache: its DTSTART and its rule.
type CadenceKey = (NaiveDate, String);

/// Each scope's settings and rules, and each distinct cadence's validation,
/// computed once per evaluation.
pub(super) struct Scopes<'c> {
	configs: &'c ConfigSet,
	/// The applied context's name, whose scoped rules are collected.
	context: Option<String>,
	directories: BTreeMap<RepoPath, DirectorySettings>,
	remotes: BTreeMap<String, RemoteSettings>,
	/// Each distinct cadence, validated once. Every schedule on a cadence
	/// shares its recurrence, and so the dates expanded from it, whatever
	/// the schedule's grace.
	cadences: BTreeMap<CadenceKey, Result<Recurrence, ScheduleError>>,
}

impl<'c> Scopes<'c> {
	pub(super) fn new(configs: &'c ConfigSet, context: Option<String>) -> Self {
		Self {
			configs,
			context,
			directories: BTreeMap::new(),
			remotes: BTreeMap::new(),
			cadences: BTreeMap::new(),
		}
	}

	/// How many cadences this evaluation has expanded: one for each distinct
	/// cadence, which every schedule on it shares.
	#[cfg(test)]
	pub(super) fn expansions(&self) -> usize {
		self.cadences.len()
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
	///
	/// Schedules on one cadence share its recurrence, whatever their grace,
	/// so the cadence is expanded once for all of them.
	pub(super) fn schedule(
		&mut self,
		settings: &EffectiveSettings,
	) -> Option<EffectiveSchedule> {
		let cadence = match &settings.cadence.value {
			Some(cadence) => Some(
				self
					.cadences
					.entry((cadence.dtstart, cadence.rrule.clone()))
					.or_insert_with(|| Recurrence::new(cadence))
					.as_ref()
					.ok()?
					.clone(),
			),
			None => None,
		};
		Some(EffectiveSchedule {
			grace: settings.grace.value,
			cadence,
		})
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
