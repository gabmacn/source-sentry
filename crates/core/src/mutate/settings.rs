//! Effective settings before and after a declaration changes projects.

mod configuration;
pub use configuration::ConfigurationChange;
pub(super) use configuration::changes as configuration_changes;

use super::{Placement, Setting, SettingsPreview};
use crate::claims::Location;
use crate::config::{ConfigSet, DocumentDefaults};
use crate::evaluate::settings::{DeclarationSite, EffectiveSettings};
use crate::model::{Declaration, DocumentId, RepoPath};

/// Resolves a declaration's settings under its project and placement.
pub(super) fn effective(
	configs: &ConfigSet,
	document: &DocumentId,
	placement: &Placement,
	declaration: &Declaration,
	location: &Location,
) -> EffectiveSettings {
	let defaults = match (document, placement) {
		(DocumentId::Local(path), _) => {
			configs
				.settings(&path.parent().unwrap_or(RepoPath::ROOT))
				.defaults
		}
		(DocumentId::Remote(_), Placement::Remote { owner, .. }) => {
			configs.remote_settings(owner).defaults
		}
		(DocumentId::Remote(_), Placement::OnFile | Placement::Lockfile { .. }) => {
			DocumentDefaults::default()
		}
	};
	EffectiveSettings::new(
		&defaults,
		Some((
			declaration,
			DeclarationSite {
				file: &location.file,
				field: location.field.as_deref().unwrap_or(""),
			},
		)),
	)
}

/// Lists each value that changes, while retaining all origins for review.
pub(super) fn preview(
	before: EffectiveSettings,
	after: EffectiveSettings,
) -> SettingsPreview {
	let changed = [
		(
			Setting::Classes,
			before.classes.value != after.classes.value,
		),
		(Setting::Owners, before.owners.value != after.owners.value),
		(Setting::Grace, before.grace.value != after.grace.value),
		(
			Setting::Cadence,
			before.cadence.value != after.cadence.value,
		),
	]
	.into_iter()
	.filter_map(|(setting, changed)| changed.then_some(setting))
	.collect();
	SettingsPreview {
		configuration: Vec::new(),
		before,
		after,
		changed,
	}
}

/// Refuses a move whose explicit declaration uses a class absent at its destination.
pub(super) fn validate_classes(
	configs: &ConfigSet,
	declaration: &Declaration,
) -> Result<(), super::MutateError> {
	for class in &declaration.classes {
		if !configs.classes().contains_key(class) {
			return Err(super::MutateError::DestinationClass {
				class: class.clone(),
				project: configs.project().clone(),
			});
		}
	}
	Ok(())
}
