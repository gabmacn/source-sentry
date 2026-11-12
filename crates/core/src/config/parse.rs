//! Strict parsing of one config file into its settings, with positions.
//!
//! The parser knows every key that any kind of config allows, so a misspelt
//! key fails at once with the list of keys this kind allows. A key that is
//! valid elsewhere, such as `root` in a preset, is reported by [`parse`] with
//! a correction that says where it belongs. Checks that need other files,
//! such as whether a class is declared, belong to [`super::load`].

use std::collections::BTreeMap;
use std::fmt;

use serde::Deserialize;
use serde::de::{self, Deserializer, Visitor};
use serde_saphyr::Spanned;

use super::error::{ConfigError, ConfigErrorKind, Position};
use super::pattern::{PathPattern, PatternError, SeriesPattern, quote};
use super::{SUPPORTED_VERSIONS, diagnose};
use crate::adapters::Adapter;
use crate::evaluate::schedule::EffectiveSchedule;
use crate::model::{
	ClassName, Context, Fact, Lifecycle, Outcome, RepoPath, Rule, Schedule,
	VersionKind,
};

/// The kinds of config, which differ in the settings they allow.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ConfigKind {
	/// `.source-sentry/source-sentry.yaml`: `version`, `time-zone`, `define`
	/// and every ordinary setting.
	Repository,
	/// `<directory>/source-sentry.yaml`: `root`, `inherit` and every ordinary
	/// setting.
	Directory,
	/// `.source-sentry/presets/<name>.yaml`: definitions and ordinary settings.
	Preset,
	/// `.source-sentry/remotes/<owner>.yaml`: only `imports`, `classes`,
	/// `owners` and `schedule`, including through its imports.
	Paired,
}

impl ConfigKind {
	/// Whether this kind of config may set the top-level `key`.
	pub(super) fn allows(self, key: &str) -> bool {
		match key {
			"version" | "time-zone" => self == Self::Repository,
			"define" => matches!(self, Self::Repository | Self::Preset),
			"root" | "inherit" => self == Self::Directory,
			"imports" | "classes" | "owners" | "schedule" => true,
			"documents" | "rules" | "series" => self != Self::Paired,
			_ => false,
		}
	}

	/// Every top-level key this kind of config allows, in the order the
	/// format describes them.
	pub(super) fn allowed_keys(self) -> Vec<&'static str> {
		TOP_LEVEL_KEYS
			.iter()
			.copied()
			.filter(|key| self.allows(key))
			.collect()
	}

	/// Where a key that this kind may not set belongs instead.
	fn belongs_elsewhere(self, key: &str) -> String {
		match (key, self) {
			("version", _) => "Only the repository config, \
			                   .source-sentry/source-sentry.yaml, sets `version`"
				.to_owned(),
			("time-zone", _) => "Set `time-zone` in the repository config, \
			                     .source-sentry/source-sentry.yaml"
				.to_owned(),
			("define", _) => "Declare classes and contexts under `define` in the \
			                  repository config, .source-sentry/source-sentry.yaml; \
			                  other configs may assign classes and add rules"
				.to_owned(),
			(_, Self::Paired) => "A paired config allows only `imports`, \
			                      `classes`, `owners` and `schedule`"
				.to_owned(),
			(_, Self::Preset) => format!(
				"Set `{key}` in the directory config that imports this preset; \
				 it applies only to its own directory"
			),
			_ => format!(
				"Set `{key}` in a directory config, <directory>/source-sentry.yaml"
			),
		}
	}
}

impl fmt::Display for ConfigKind {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		formatter.write_str(match self {
			Self::Repository => "repository config",
			Self::Directory => "directory config",
			Self::Preset => "preset",
			Self::Paired => "paired config",
		})
	}
}

/// Every top-level key of any config.
const TOP_LEVEL_KEYS: &[&str] = &[
	"version",
	"time-zone",
	"define",
	"root",
	"inherit",
	"imports",
	"classes",
	"owners",
	"schedule",
	"documents",
	"rules",
	"series",
];

/// One entry of `documents.adapters`: the files a pattern matches are read with
/// an adapter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdapterMapping {
	/// The files the mapping applies to.
	pub pattern: PathPattern,
	/// How they are read.
	pub adapter: Adapter,
}

/// A value with the position it was written at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Located<T> {
	/// The value.
	pub(super) value: T,
	/// Where its value starts in the file.
	pub(super) position: Position,
}

/// One config file, parsed and checked on its own.
///
/// A setting that the file does not write is `None`, or empty for
/// `imports`. Every list item keeps its position for later diagnostics.
#[derive(Clone, Debug)]
pub(super) struct Parsed {
	pub(super) path: RepoPath,
	/// Field positions retained for checks that need other config files.
	pub(super) field_positions: BTreeMap<String, Position>,
	pub(super) version: Option<Located<u32>>,
	pub(super) time_zone: Option<Located<String>>,
	pub(super) define: Option<Define>,
	pub(super) root: Option<Located<bool>>,
	pub(super) inherit: Option<Located<bool>>,
	pub(super) imports: Vec<Located<String>>,
	pub(super) classes: Option<Located<Vec<Located<ClassName>>>>,
	pub(super) owners: Option<Located<Vec<String>>>,
	pub(super) schedule: Option<Located<Schedule>>,
	/// Where `schedule.grace` and `schedule.cadence` themselves are, when
	/// they are written, so that their origins name their own lines rather
	/// than the `schedule` mapping's.
	pub(super) grace_position: Option<Position>,
	pub(super) cadence_position: Option<Position>,
	pub(super) documents: Option<Located<Documents>>,
	pub(super) rules: Option<Located<RuleMap>>,
	pub(super) series:
		Option<Located<BTreeMap<String, Located<SeriesDefinition>>>>,
}

/// Rules added to contexts, by context name.
pub(super) type RuleMap = BTreeMap<String, Located<Vec<Located<Rule>>>>;

/// The repository config's `define`.
#[derive(Clone, Debug, Default)]
pub(super) struct Define {
	/// Declared classes, with their optional descriptions.
	pub(super) classes: BTreeMap<ClassName, Located<Option<String>>>,
	/// Declared contexts.
	pub(super) contexts: BTreeMap<String, Located<DefinedContext>>,
}

/// A declared context, with where each of its rules starts.
#[derive(Clone, Debug)]
pub(super) struct DefinedContext {
	/// The context.
	pub(super) context: Context,
	/// The position of each of `context.rules`, in order.
	pub(super) rules: Vec<Position>,
}

/// A config's `documents`.
#[derive(Clone, Debug, Default)]
pub(super) struct Documents {
	pub(super) include: Option<Located<Vec<Located<PathPattern>>>>,
	pub(super) ignore: Option<Located<Vec<Located<PathPattern>>>>,
	pub(super) adapters: Option<Located<Vec<Located<AdapterMapping>>>>,
}

/// One series as a config defines it.
#[derive(Clone, Debug)]
pub(super) struct SeriesDefinition {
	pub(super) pattern: SeriesPattern,
	pub(super) version_kind: VersionKind,
	pub(super) lifecycle: Lifecycle,
	pub(super) schedule: Option<Schedule>,
	/// Where the series writes its cadence, for errors after resolution.
	pub(super) cadence_position: Option<Position>,
}

/// Parses and checks one config file of the given kind.
///
/// # Errors
///
/// Returns every mistake found in the file on its own: YAML and type errors
/// (only the first one, because parsing stops there), keys this kind may not
/// set, empty values, unsupported versions, invalid names and patterns, and
/// rules that target `fresh`.
pub(super) fn parse(
	path: &RepoPath,
	kind: ConfigKind,
	bytes: &[u8],
) -> Result<Parsed, Vec<ConfigError>> {
	let Ok(text) = std::str::from_utf8(bytes) else {
		return Err(vec![ConfigError::new(path, ConfigErrorKind::NotUtf8)]);
	};
	let raw = diagnose::deserialize::<RawFile>(path, kind, text)
		.map_err(|error| vec![*error])?;
	let mut converter = Converter {
		path,
		kind,
		errors: Vec::new(),
		lines: crate::yaml::field_lines(text).unwrap_or_default(),
	};
	let parsed = converter.file(raw);
	if converter.errors.is_empty() {
		Ok(parsed)
	} else {
		Err(converter.errors)
	}
}

// The raw shapes that YAML deserializes into.
//
// `diagnose::render_setting` relies on an invariant here: every struct field,
// map value and list item of these raw types is wrapped in `Field` or `Item`,
// which are spanned, while the model types they hold (`Rule`, `Context`,
// `Schedule` and the enums) are not. Keep it when adding a setting.

/// A setting as written: absent, explicitly empty, or present.
#[derive(Debug, Default)]
enum Field<T> {
	#[default]
	Absent,
	Null(Position),
	Value(Located<T>),
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Field<T> {
	fn deserialize<D: Deserializer<'de>>(
		deserializer: D,
	) -> Result<Self, D::Error> {
		let spanned = Spanned::<Option<T>>::deserialize(deserializer)?;
		let position = position_of(&spanned.referenced);
		Ok(match spanned.value {
			None => Self::Null(position),
			Some(value) => Self::Value(Located { value, position }),
		})
	}
}

/// A list item or map value, with its position.
#[derive(Debug)]
struct Item<T>(Located<T>);

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Item<T> {
	fn deserialize<D: Deserializer<'de>>(
		deserializer: D,
	) -> Result<Self, D::Error> {
		let spanned = Spanned::<T>::deserialize(deserializer)?;
		Ok(Self(Located {
			position: position_of(&spanned.referenced),
			value: spanned.value,
		}))
	}
}

/// Converts serde-saphyr's location to a position.
pub(super) fn position_of(location: &serde_saphyr::Location) -> Position {
	Position {
		line: location.line(),
		column: location.column(),
	}
}

/// What the whole-number visitor expects. `diagnose` recognizes its errors
/// by this text.
pub(super) const WHOLE_NUMBER: &str = "a whole number, such as `1`";

/// A whole number written as a YAML integer, never as a float or a string.
#[derive(Debug)]
struct WholeNumber(u32);

impl<'de> Deserialize<'de> for WholeNumber {
	fn deserialize<D: Deserializer<'de>>(
		deserializer: D,
	) -> Result<Self, D::Error> {
		struct WholeNumberVisitor;

		impl Visitor<'_> for WholeNumberVisitor {
			type Value = WholeNumber;

			fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
				formatter.write_str(WHOLE_NUMBER)
			}

			fn visit_u64<E: de::Error>(self, value: u64) -> Result<WholeNumber, E> {
				u32::try_from(value)
					.map(WholeNumber)
					.map_err(|_| E::invalid_value(de::Unexpected::Unsigned(value), &self))
			}

			fn visit_i64<E: de::Error>(self, value: i64) -> Result<WholeNumber, E> {
				u32::try_from(value)
					.map(WholeNumber)
					.map_err(|_| E::invalid_value(de::Unexpected::Signed(value), &self))
			}
		}

		deserializer.deserialize_any(WholeNumberVisitor)
	}
}

/// What the strict-string visitor expects.
const TIME_ZONE_NAME: &str = "an IANA time zone name, such as 'Europe/Madrid'";

/// A string written as a YAML string, never a number or boolean that the
/// parser would otherwise turn into text.
#[derive(Debug)]
struct StrictString(String);

impl<'de> Deserialize<'de> for StrictString {
	fn deserialize<D: Deserializer<'de>>(
		deserializer: D,
	) -> Result<Self, D::Error> {
		struct StrictStringVisitor;

		impl Visitor<'_> for StrictStringVisitor {
			type Value = StrictString;

			fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
				formatter.write_str(TIME_ZONE_NAME)
			}

			fn visit_str<E: de::Error>(self, value: &str) -> Result<StrictString, E> {
				Ok(StrictString(value.to_owned()))
			}
		}

		deserializer.deserialize_any(StrictStringVisitor)
	}
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RawFile {
	#[serde(default)]
	version: Field<WholeNumber>,
	#[serde(default)]
	time_zone: Field<StrictString>,
	#[serde(default)]
	define: Field<RawDefine>,
	#[serde(default)]
	root: Field<bool>,
	#[serde(default)]
	inherit: Field<bool>,
	#[serde(default)]
	imports: Field<Vec<Item<String>>>,
	#[serde(default)]
	classes: Field<Vec<Item<String>>>,
	#[serde(default)]
	owners: Field<Vec<Item<String>>>,
	#[serde(default)]
	schedule: Field<Schedule>,
	#[serde(default)]
	documents: Field<RawDocuments>,
	#[serde(default)]
	rules: Field<BTreeMap<String, Item<Vec<Item<Rule>>>>>,
	#[serde(default)]
	series: Field<BTreeMap<String, Item<RawSeries>>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RawDefine {
	#[serde(default)]
	classes: Field<BTreeMap<String, Item<Option<RawClass>>>>,
	#[serde(default)]
	contexts: Field<BTreeMap<String, Item<RawContext>>>,
}

/// A context as written, keeping each rule's position.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RawContext {
	fallback: Outcome,
	#[serde(default)]
	rules: Vec<Item<Rule>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RawClass {
	#[serde(default)]
	description: Field<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RawDocuments {
	#[serde(default)]
	include: Field<Vec<Item<String>>>,
	#[serde(default)]
	ignore: Field<Vec<Item<String>>>,
	#[serde(default)]
	adapters: Field<Vec<Item<RawAdapter>>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RawAdapter {
	#[serde(default, rename = "match")]
	pattern: Field<String>,
	#[serde(default)]
	adapter: Field<Adapter>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RawSeries {
	#[serde(default)]
	pattern: Field<String>,
	#[serde(default)]
	version_kind: Field<VersionKind>,
	#[serde(default)]
	lifecycle: Field<Lifecycle>,
	#[serde(default)]
	schedule: Field<Schedule>,
}

/// Turns raw shapes into checked settings, collecting every error.
struct Converter<'a> {
	path: &'a RepoPath,
	kind: ConfigKind,
	errors: Vec<ConfigError>,
	/// Where each field starts, as a line and a column, by its path, such as
	/// `schedule.cadence`.
	lines: BTreeMap<String, (u64, u64)>,
}

impl Converter<'_> {
	/// Where `field` starts, if the file writes it.
	fn position_of(&self, field: &str) -> Option<Position> {
		self
			.lines
			.get(field)
			.map(|&(line, column)| Position { line, column })
	}

	/// Records an error at `position` in `setting`.
	fn report(
		&mut self,
		kind: ConfigErrorKind,
		setting: &str,
		position: Position,
		correction: Option<String>,
	) {
		let mut error = ConfigError::new(self.path, kind)
			.at(Some(position))
			.in_setting(setting);
		error.correction = correction;
		self.errors.push(error);
	}

	/// The value of `field`, or `None` when it is absent or empty. An empty value
	/// is an error, because it would reset nothing.
	fn value<T>(&mut self, field: Field<T>, setting: &str) -> Option<Located<T>> {
		match field {
			Field::Absent => None,
			Field::Null(position) => {
				self.report(
					ConfigErrorKind::EmptyValue,
					setting,
					position,
					Some(format!(
						"Remove `{setting}`, or give it a value; an empty value resets \
						 nothing, because settings inherit one by one"
					)),
				);
				None
			}
			Field::Value(value) => Some(value),
		}
	}

	/// The value of the top-level setting `key`, reporting it instead when
	/// this kind of config may not set it.
	fn top<T>(&mut self, field: Field<T>, key: &str) -> Option<Located<T>> {
		let position = match &field {
			Field::Absent => return None,
			Field::Null(position) => *position,
			Field::Value(value) => value.position,
		};
		if self.kind.allows(key) {
			return self.value(field, key);
		}
		self.report(
			ConfigErrorKind::ForbiddenSetting {
				key: key.to_owned(),
				kind: self.kind,
			},
			key,
			position,
			Some(self.kind.belongs_elsewhere(key)),
		);
		None
	}

	fn file(&mut self, raw: RawFile) -> Parsed {
		let version_absent = matches!(raw.version, Field::Absent);
		let version = self.top(raw.version, "version").map(|version| Located {
			value: version.value.0,
			position: version.position,
		});
		self.check_version(version.as_ref(), version_absent);
		let time_zone = self.time_zone(raw.time_zone);
		let define = self
			.top(raw.define, "define")
			.map(|define| self.define(define));
		let root = self.top(raw.root, "root");
		let inherit = self.top(raw.inherit, "inherit");
		let imports = self.top(raw.imports, "imports");
		let imports =
			imports.map_or_else(Vec::new, |imports| self.imports(imports.value));
		let classes = self.top(raw.classes, "classes");
		let classes = classes.map(|classes| self.classes(classes));
		let owners = self.top(raw.owners, "owners");
		let owners = owners.map(|owners| self.owners(owners));
		let schedule = self.top(raw.schedule, "schedule");
		if let Some(schedule) = &schedule {
			self.schedule(&schedule.value, "schedule", schedule.position);
		}
		let documents = self.top(raw.documents, "documents");
		let documents = documents.map(|documents| Located {
			position: documents.position,
			value: self.documents(documents.value),
		});
		let rules = self.top(raw.rules, "rules");
		let rules = rules.map(|rules| Located {
			position: rules.position,
			value: self.rules(rules.value),
		});
		let series = self.top(raw.series, "series");
		let series = series.map(|series| Located {
			position: series.position,
			value: self.series(series.value),
		});
		Parsed {
			path: self.path.clone(),
			version,
			time_zone,
			define,
			root,
			inherit,
			imports,
			classes,
			owners,
			grace_position: schedule
				.as_ref()
				.and_then(|_| self.position_of("schedule.grace")),
			cadence_position: schedule
				.as_ref()
				.and_then(|_| self.position_of("schedule.cadence")),
			schedule,
			documents,
			rules,
			series,
			field_positions: std::mem::take(&mut self.lines)
				.into_iter()
				.map(|(field, (line, column))| (field, Position { line, column }))
				.collect(),
		}
	}

	/// The repository config must pin a supported version.
	fn check_version(&mut self, version: Option<&Located<u32>>, absent: bool) {
		if self.kind != ConfigKind::Repository {
			return;
		}
		match version {
			Some(version) if !SUPPORTED_VERSIONS.contains(&version.value) => {
				self.report(
					ConfigErrorKind::UnsupportedVersion {
						version: version.value,
					},
					"version",
					version.position,
					Some(format!("Use {}", supported_versions())),
				);
			}
			None if absent => {
				let mut error = ConfigError::new(
					self.path,
					ConfigErrorKind::MissingSetting {
						key: "version".to_owned(),
					},
				)
				.in_setting("version");
				error.correction = Some(format!(
					"Add {}; it pins the schema and the default policy",
					supported_versions()
				));
				self.errors.push(error);
			}
			_ => {}
		}
	}

	/// Reports a cadence that is not a valid recurrence rule, whether or not
	/// any document uses it yet.
	fn schedule(
		&mut self,
		schedule: &Schedule,
		setting: &str,
		position: Position,
	) {
		if let Err(error) = EffectiveSchedule::new(schedule) {
			let field = format!("{setting}.cadence");
			let position = self.position_of(&field).unwrap_or(position);
			self.report(
				ConfigErrorKind::InvalidSchedule {
					reason: error.to_string(),
				},
				&field,
				position,
				None,
			);
		}
	}

	fn time_zone(
		&mut self,
		field: Field<StrictString>,
	) -> Option<Located<String>> {
		let time_zone = self.top(field, "time-zone")?;
		if time_zone.value.0.trim().is_empty() {
			self.report(
				ConfigErrorKind::EmptyValue,
				"time-zone",
				time_zone.position,
				Some(
					"Use an IANA name, such as 'Europe/Madrid', or remove `time-zone` \
					 to use UTC"
						.to_owned(),
				),
			);
			return None;
		}
		Some(Located {
			value: time_zone.value.0,
			position: time_zone.position,
		})
	}

	fn define(&mut self, define: Located<RawDefine>) -> Define {
		let mut checked = Define::default();
		let classes = self.value(define.value.classes, "define.classes");
		for (name, item) in classes.map(|classes| classes.value).unwrap_or_default()
		{
			let setting = format!("define.classes.{name}");
			let Item(located) = item;
			let description = match located.value {
				Some(class) => self
					.value(class.description, &format!("{setting}.description"))
					.map(|description| description.value),
				None => None,
			};
			if let Some(class) = self.class_name(&name, &setting, located.position) {
				checked.classes.insert(
					class,
					Located {
						value: description,
						position: located.position,
					},
				);
			}
		}
		let contexts = self.value(define.value.contexts, "define.contexts");
		for (name, Item(context)) in
			contexts.map(|contexts| contexts.value).unwrap_or_default()
		{
			let setting = format!("define.contexts.{name}");
			self.name(&name, "context name", &setting, context.position);
			let mut rules = Vec::with_capacity(context.value.rules.len());
			let mut positions = Vec::with_capacity(context.value.rules.len());
			for (index, Item(rule)) in context.value.rules.into_iter().enumerate() {
				if rule.value.facts.contains(&Fact::Fresh) {
					let field = format!("{setting}.rules[{index}].facts");
					let position = self.position_of(&field).unwrap_or(rule.position);
					self.fresh(&field, position);
				}
				positions.push(rule.position);
				rules.push(rule.value);
			}
			checked.contexts.insert(
				name,
				Located {
					value: DefinedContext {
						context: Context {
							fallback: context.value.fallback,
							rules,
						},
						rules: positions,
					},
					position: context.position,
				},
			);
		}
		checked
	}

	fn imports(&mut self, imports: Vec<Item<String>>) -> Vec<Located<String>> {
		let mut checked = Vec::with_capacity(imports.len());
		for (index, Item(import)) in imports.into_iter().enumerate() {
			if let Err((reason, correction)) = check_import(&import.value) {
				self.report(
					ConfigErrorKind::InvalidImport {
						name: import.value.clone(),
						reason,
					},
					&format!("imports[{index}]"),
					import.position,
					correction,
				);
			}
			checked.push(import);
		}
		checked
	}

	fn classes(
		&mut self,
		classes: Located<Vec<Item<String>>>,
	) -> Located<Vec<Located<ClassName>>> {
		let mut checked = Vec::with_capacity(classes.value.len());
		for (index, Item(class)) in classes.value.into_iter().enumerate() {
			let setting = format!("classes[{index}]");
			if let Some(name) =
				self.class_name(&class.value, &setting, class.position)
			{
				checked.push(Located {
					value: name,
					position: class.position,
				});
			}
		}
		Located {
			value: checked,
			position: classes.position,
		}
	}

	fn owners(
		&mut self,
		owners: Located<Vec<Item<String>>>,
	) -> Located<Vec<String>> {
		let mut checked = Vec::with_capacity(owners.value.len());
		for (index, Item(owner)) in owners.value.into_iter().enumerate() {
			if owner.value.trim().is_empty() {
				self.report(
					ConfigErrorKind::InvalidName {
						name: owner.value.clone(),
						what: "owner identifier",
					},
					&format!("owners[{index}]"),
					owner.position,
					Some("Name an owner, such as 'legal'".to_owned()),
				);
			}
			checked.push(owner.value);
		}
		Located {
			value: checked,
			position: owners.position,
		}
	}

	fn documents(&mut self, documents: RawDocuments) -> Documents {
		Documents {
			include: self
				.value(documents.include, "documents.include")
				.map(|include| self.patterns(include, "documents.include", false)),
			ignore: self
				.value(documents.ignore, "documents.ignore")
				.map(|ignore| self.patterns(ignore, "documents.ignore", true)),
			adapters: self
				.value(documents.adapters, "documents.adapters")
				.map(|adapters| self.adapters(adapters)),
		}
	}

	fn patterns(
		&mut self,
		patterns: Located<Vec<Item<String>>>,
		setting: &str,
		allow_negation: bool,
	) -> Located<Vec<Located<PathPattern>>> {
		let mut checked = Vec::with_capacity(patterns.value.len());
		for (index, Item(pattern)) in patterns.value.into_iter().enumerate() {
			let setting = format!("{setting}[{index}]");
			if let Some(compiled) =
				self.path_pattern(&pattern, &setting, allow_negation)
			{
				checked.push(Located {
					value: compiled,
					position: pattern.position,
				});
			}
		}
		Located {
			value: checked,
			position: patterns.position,
		}
	}

	fn adapters(
		&mut self,
		adapters: Located<Vec<Item<RawAdapter>>>,
	) -> Located<Vec<Located<AdapterMapping>>> {
		let mut checked = Vec::with_capacity(adapters.value.len());
		for (index, Item(mapping)) in adapters.value.into_iter().enumerate() {
			let setting = format!("documents.adapters[{index}]");
			let pattern = self.required(
				mapping.value.pattern,
				&format!("{setting}.match"),
				mapping.position,
			);
			let adapter = self.required(
				mapping.value.adapter,
				&format!("{setting}.adapter"),
				mapping.position,
			);
			let pattern = pattern.and_then(|pattern| {
				self.path_pattern(&pattern, &format!("{setting}.match"), false)
			});
			if let (Some(pattern), Some(adapter)) = (pattern, adapter) {
				checked.push(Located {
					value: AdapterMapping {
						pattern,
						adapter: adapter.value,
					},
					position: mapping.position,
				});
			}
		}
		Located {
			value: checked,
			position: adapters.position,
		}
	}

	fn rules(
		&mut self,
		rules: BTreeMap<String, Item<Vec<Item<Rule>>>>,
	) -> RuleMap {
		let mut checked = BTreeMap::new();
		for (context, Item(list)) in rules {
			let setting = format!("rules.{context}");
			self.name(&context, "context name", &setting, list.position);
			let mut kept = Vec::with_capacity(list.value.len());
			for (index, Item(rule)) in list.value.into_iter().enumerate() {
				if rule.value.facts.contains(&Fact::Fresh) {
					let field = format!("{setting}[{index}].facts");
					let position = self.position_of(&field).unwrap_or(rule.position);
					self.fresh(&field, position);
				}
				// A context's own rules get this check when the context is
				// validated; scoped rules only here.
				if rule.value.facts.is_empty() {
					let field = format!("{setting}[{index}].facts");
					let position = self.position_of(&field).unwrap_or(rule.position);
					self.report(
						ConfigErrorKind::NoFacts,
						&field,
						position,
						Some("List the facts the rule applies to".to_owned()),
					);
				}
				kept.push(rule);
			}
			checked.insert(
				context,
				Located {
					value: kept,
					position: list.position,
				},
			);
		}
		checked
	}

	fn series(
		&mut self,
		series: BTreeMap<String, Item<RawSeries>>,
	) -> BTreeMap<String, Located<SeriesDefinition>> {
		let mut checked = BTreeMap::new();
		for (name, Item(raw)) in series {
			let setting = format!("series.{name}");
			let named = self.name(&name, "series name", &setting, raw.position);
			let position = raw.position;
			let pattern = self.required(
				raw.value.pattern,
				&format!("{setting}.pattern"),
				position,
			);
			let version_kind = self.required(
				raw.value.version_kind,
				&format!("{setting}.version-kind"),
				position,
			);
			let lifecycle = self.required(
				raw.value.lifecycle,
				&format!("{setting}.lifecycle"),
				position,
			);
			let schedule = self
				.value(raw.value.schedule, &format!("{setting}.schedule"))
				.map(|schedule| {
					self.schedule(
						&schedule.value,
						&format!("{setting}.schedule"),
						schedule.position,
					);
					schedule.value
				});
			let pattern = pattern.and_then(|pattern| {
				self.series_pattern(&pattern, &format!("{setting}.pattern"))
			});
			if let (true, Some(pattern), Some(version_kind), Some(lifecycle)) =
				(named, pattern, version_kind, lifecycle)
			{
				checked.insert(
					name,
					Located {
						value: SeriesDefinition {
							pattern,
							version_kind: version_kind.value,
							lifecycle: lifecycle.value,
							cadence_position: self
								.position_of(&format!("{setting}.schedule.cadence")),
							schedule,
						},
						position,
					},
				);
			}
		}
		checked
	}

	/// The value of a required nested field, reporting it when it is absent.
	/// `parent` is where the enclosing mapping starts.
	fn required<T>(
		&mut self,
		field: Field<T>,
		setting: &str,
		parent: Position,
	) -> Option<Located<T>> {
		if matches!(field, Field::Absent) {
			let key = setting.rsplit('.').next().unwrap_or(setting).to_owned();
			self.report(
				ConfigErrorKind::MissingSetting { key: key.clone() },
				setting,
				parent,
				Some(format!("Add `{key}`")),
			);
			return None;
		}
		self.value(field, setting)
	}

	fn path_pattern(
		&mut self,
		pattern: &Located<String>,
		setting: &str,
		allow_negation: bool,
	) -> Option<PathPattern> {
		PathPattern::compile(&pattern.value, allow_negation)
			.map_err(|error| self.invalid_pattern(pattern, setting, error, false))
			.ok()
	}

	fn series_pattern(
		&mut self,
		pattern: &Located<String>,
		setting: &str,
	) -> Option<SeriesPattern> {
		SeriesPattern::compile(&pattern.value)
			.map_err(|error| self.invalid_pattern(pattern, setting, error, true))
			.ok()
	}

	fn invalid_pattern(
		&mut self,
		pattern: &Located<String>,
		setting: &str,
		error: PatternError,
		series: bool,
	) {
		let kind = if series {
			ConfigErrorKind::InvalidSeriesPattern {
				pattern: pattern.value.clone(),
				reason: error.reason,
			}
		} else {
			ConfigErrorKind::InvalidPattern {
				pattern: pattern.value.clone(),
				reason: error.reason,
			}
		};
		self.report(kind, setting, pattern.position, error.correction);
	}

	/// Checks a class name, reporting it when it is not kebab-case.
	fn class_name(
		&mut self,
		name: &str,
		setting: &str,
		position: Position,
	) -> Option<ClassName> {
		let class = ClassName::new(name).ok();
		if class.is_none() {
			self.invalid_name(name, "class name", setting, position);
		}
		class
	}

	/// Checks a context or series name, which follow the same rule as class
	/// names. Returns whether it is valid.
	fn name(
		&mut self,
		name: &str,
		what: &'static str,
		setting: &str,
		position: Position,
	) -> bool {
		let valid = ClassName::new(name).is_ok();
		if !valid {
			self.invalid_name(name, what, setting, position);
		}
		valid
	}

	fn invalid_name(
		&mut self,
		name: &str,
		what: &'static str,
		setting: &str,
		position: Position,
	) {
		self.report(
			ConfigErrorKind::InvalidName {
				name: name.to_owned(),
				what,
			},
			setting,
			position,
			Some(format!(
				"Use lowercase kebab-case, such as {}",
				quote(&kebab_case(name))
			)),
		);
	}

	fn fresh(&mut self, setting: &str, position: Position) {
		self.report(
			ConfigErrorKind::FreshRule,
			setting,
			position,
			Some("Remove `fresh` from `facts`".to_owned()),
		);
	}
}

/// Every supported version, as a correction would write it.
fn supported_versions() -> String {
	SUPPORTED_VERSIONS
		.iter()
		.map(|version| format!("`version: {version}`"))
		.collect::<Vec<_>>()
		.join(" or ")
}

/// A guess at the kebab-case spelling of `name`, for corrections.
fn kebab_case(name: &str) -> String {
	let mut kebab = String::with_capacity(name.len());
	for character in name.trim().chars() {
		if character.is_ascii_alphanumeric() {
			if character.is_ascii_uppercase()
				&& !kebab.is_empty()
				&& !kebab.ends_with('-')
			{
				kebab.push('-');
			}
			kebab.push(character.to_ascii_lowercase());
		} else if !kebab.is_empty() && !kebab.ends_with('-') {
			kebab.push('-');
		}
	}
	let kebab = kebab.trim_end_matches('-').to_owned();
	if kebab.is_empty() {
		"my-name".to_owned()
	} else {
		kebab
	}
}

/// Checks an import identifier: a preset's path inside
/// `.source-sentry/presets/`, without an extension.
fn check_import(name: &str) -> Result<(), (String, Option<String>)> {
	if let Some(stem) = name
		.strip_suffix(".yaml")
		.or_else(|| name.strip_suffix(".yml"))
	{
		return Err((
			"imports name presets without an extension".to_owned(),
			Some(format!("Use {}", quote(stem))),
		));
	}
	if name.is_empty() {
		return Err(("the name is empty".to_owned(), None));
	}
	if name.starts_with('/') {
		return Err((
			"preset names are relative to .source-sentry/presets/".to_owned(),
			Some(format!("Use {}", quote(name.trim_start_matches('/')))),
		));
	}
	if name.contains('\\') {
		return Err((
			"preset names separate folders with `/`".to_owned(),
			Some(format!("Use {}", quote(&name.replace('\\', "/")))),
		));
	}
	if name
		.split('/')
		.any(|part| part.is_empty() || part == "." || part == "..")
	{
		return Err((
			"every folder and the name must be non-empty, and `.` and `..` are not \
			 allowed, so that imports stay inside .source-sentry/presets/"
				.to_owned(),
			None,
		));
	}
	if RepoPath::new(name).is_err() {
		return Err(("the name is not a valid path".to_owned(), None));
	}
	Ok(())
}
