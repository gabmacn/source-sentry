//! A document's effective settings: its declaration's own fields over the
//! defaults of its directory or remote owner.
//!
//! The config module resolves defaults per directory and per remote owner,
//! but its origins cannot name a document's own declaration. This module adds
//! that last layer, so evaluation, the mutation commands and the report all
//! agree on what applies to a document and why.

use crate::config::{DocumentDefaults, Origin};
use crate::model::{
	Cadence, CadenceSetting, ClassName, Declaration, RepoPath, Schedule,
};

/// Where an effective setting comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettingOrigin {
	/// Nothing sets it, so the built-in value applies: no classes, no owners,
	/// a grace of 0 days and no cadence.
	BuiltIn,
	/// A config or preset, through resolution.
	Config(Origin),
	/// The document's own declaration.
	Declaration {
		/// The file holding the declaration: the document itself when
		/// on-file, otherwise its lockfile.
		file: RepoPath,
		/// The field that sets the value, such as
		/// `source-sentry.schedule.grace` or
		/// `documents["a.md"].classes`.
		field: String,
	},
}

/// A setting's value and where it comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Effective<T> {
	/// The value.
	pub value: T,
	/// Where it comes from.
	pub origin: SettingOrigin,
}

/// Where a declaration is written, which its origins name.
#[derive(Clone, Copy, Debug)]
pub struct DeclarationSite<'a> {
	/// The file holding the declaration.
	pub file: &'a RepoPath,
	/// The declaration's own field in that file: `source-sentry` in
	/// frontmatter, or `documents["<key>"]` in a lockfile.
	pub field: &'a str,
}

impl DeclarationSite<'_> {
	fn origin(&self, setting: &str) -> SettingOrigin {
		SettingOrigin::Declaration {
			file: self.file.clone(),
			field: format!("{}.{setting}", self.field),
		}
	}
}

/// The settings that apply to one document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectiveSettings {
	/// The document's classes, which decide the context rules that apply.
	pub classes: Effective<Vec<ClassName>>,
	/// Owner identifiers.
	pub owners: Effective<Vec<String>>,
	/// Whole days the document may lag behind a changed source.
	pub grace: Effective<u32>,
	/// The cadence that due dates fall on, if any.
	pub cadence: Effective<Option<Cadence>>,
}

impl EffectiveSettings {
	/// The effective settings of a document.
	///
	/// `defaults` come from the document's directory, or from its remote
	/// owner. `declaration` is the document's own declaration and where it is
	/// written; pass `None` for an unclaimed file, which has none.
	///
	/// Each field of a declaration replaces the default as a whole: a
	/// non-empty `classes` or `owners` list, a `grace`, or a `cadence`. An
	/// empty list is an absent one, as the formats omit it. `cadence: null`
	/// removes an inherited cadence.
	#[must_use]
	pub fn new(
		defaults: &DocumentDefaults,
		declaration: Option<(&Declaration, DeclarationSite<'_>)>,
	) -> Self {
		let mut settings = Self::defaults(defaults);
		let Some((declaration, site)) = declaration else {
			return settings;
		};
		if !declaration.classes.is_empty() {
			settings.classes = Effective {
				value: declaration.classes.clone(),
				origin: site.origin("classes"),
			};
		}
		if !declaration.owners.is_empty() {
			settings.owners = Effective {
				value: declaration.owners.clone(),
				origin: site.origin("owners"),
			};
		}
		if let Some(schedule) = &declaration.schedule {
			if let Some(grace) = schedule.grace {
				settings.grace = Effective {
					value: grace,
					origin: site.origin("schedule.grace"),
				};
			}
			if !schedule.cadence.is_inherit() {
				settings.cadence = Effective {
					value: schedule.cadence.as_set().cloned(),
					origin: site.origin("schedule.cadence"),
				};
			}
		}
		settings
	}

	/// The settings of a document with no declaration of its own.
	fn defaults(defaults: &DocumentDefaults) -> Self {
		fn from_config<T: Clone>(
			resolved: Option<&crate::config::Resolved<T>>,
			built_in: T,
		) -> Effective<T> {
			match resolved {
				Some(resolved) => Effective {
					value: resolved.value.clone(),
					origin: SettingOrigin::Config(resolved.origin.clone()),
				},
				None => Effective {
					value: built_in,
					origin: SettingOrigin::BuiltIn,
				},
			}
		}

		Self {
			classes: from_config(defaults.classes.as_ref(), Vec::new()),
			owners: from_config(defaults.owners.as_ref(), Vec::new()),
			grace: from_config(defaults.grace.as_ref(), 0),
			cadence: from_config(defaults.cadence.as_ref(), None),
		}
	}

	/// The schedule as a model value, ready for
	/// [`EffectiveSchedule::new`](super::schedule::EffectiveSchedule::new).
	#[must_use]
	pub fn schedule(&self) -> Schedule {
		Schedule {
			cadence: match &self.cadence.value {
				Some(cadence) => CadenceSetting::Set(cadence.clone()),
				None => CadenceSetting::Remove,
			},
			grace: Some(self.grace.value),
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::config::{Resolved, Scope};
	use crate::testing::path;

	fn origin(file: &str) -> Origin {
		Origin {
			file: Some(path(file)),
			position: None,
			import_chain: Vec::new(),
			scope: Scope::Repository,
		}
	}

	fn cadence(rule: &str) -> Cadence {
		Cadence {
			dtstart: "2026-12-01".parse().unwrap(),
			rrule: rule.to_owned(),
		}
	}

	fn defaults() -> DocumentDefaults {
		DocumentDefaults {
			classes: Some(Resolved {
				value: vec![ClassName::new("internal-docs").unwrap()],
				origin: origin("docs/source-sentry.yaml"),
			}),
			owners: None,
			grace: Some(Resolved {
				value: 14,
				origin: origin(".source-sentry/source-sentry.yaml"),
			}),
			cadence: Some(Resolved {
				value: Some(cadence("FREQ=MONTHLY")),
				origin: origin(".source-sentry/source-sentry.yaml"),
			}),
		}
	}

	#[test]
	fn without_a_declaration_the_defaults_apply() {
		let settings = EffectiveSettings::new(&defaults(), None);
		assert_eq!(settings.classes.value[0].as_str(), "internal-docs");
		assert_eq!(settings.owners.value, Vec::<String>::new());
		assert_eq!(settings.owners.origin, SettingOrigin::BuiltIn);
		assert_eq!(settings.grace.value, 14);
		assert_eq!(settings.cadence.value, Some(cadence("FREQ=MONTHLY")));
	}

	#[test]
	fn declaration_fields_replace_defaults_one_by_one() {
		let lockfile = path("docs/source-sentry.lock");
		let declaration = Declaration {
			owners: vec!["legal".to_owned()],
			schedule: Some(Schedule {
				cadence: CadenceSetting::Inherit,
				grace: Some(0),
			}),
			..Declaration::default()
		};
		let site = DeclarationSite {
			file: &lockfile,
			field: "documents[\"a.md\"]",
		};
		let settings =
			EffectiveSettings::new(&defaults(), Some((&declaration, site)));
		// No classes in the declaration, so the directory's apply.
		assert_eq!(settings.classes.value[0].as_str(), "internal-docs");
		assert_eq!(settings.owners.value, ["legal"]);
		assert_eq!(
			settings.owners.origin,
			SettingOrigin::Declaration {
				file: lockfile.clone(),
				field: "documents[\"a.md\"].owners".to_owned(),
			}
		);
		assert_eq!(settings.grace.value, 0);
		// An absent cadence inherits.
		assert_eq!(settings.cadence.value, Some(cadence("FREQ=MONTHLY")));
	}

	/// `cadence: null` in a declaration removes the inherited
	/// cadence.
	#[test]
	fn a_null_cadence_removes_the_inherited_one() {
		let document = path("docs/a.md");
		let declaration = Declaration {
			schedule: Some(Schedule {
				cadence: CadenceSetting::Remove,
				grace: None,
			}),
			..Declaration::default()
		};
		let site = DeclarationSite {
			file: &document,
			field: "source-sentry",
		};
		let settings =
			EffectiveSettings::new(&defaults(), Some((&declaration, site)));
		assert_eq!(settings.cadence.value, None);
		assert_eq!(settings.grace.value, 14);
		assert_eq!(
			settings.schedule(),
			Schedule {
				cadence: CadenceSetting::Remove,
				grace: Some(14),
			}
		);
	}
}
