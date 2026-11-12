//! Configuration errors: what is wrong, where, and how to fix it.

use std::fmt;

use super::ConfigKind;
use crate::model::RepoPath;
use crate::ports::BackendError;

/// A position in a config file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Position {
	/// The line, counted from 1.
	pub line: u64,
	/// The column, in characters, counted from 1.
	pub column: u64,
}

impl fmt::Display for Position {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(formatter, "{}:{}", self.line, self.column)
	}
}

/// Why [`ConfigSet::load`](super::ConfigSet::load) failed.
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
	/// The snapshot could not be read, so the evaluation could not complete (exit
	/// 4).
	#[error(transparent)]
	Backend(#[from] BackendError),
	/// At least one config is invalid. These are configuration errors, which no
	/// context can downgrade (exit 3). They are sorted by file and position.
	#[error("{}", render_all(.0))]
	Invalid(Vec<ConfigError>),
}

/// Renders every error on its own line.
fn render_all(errors: &[ConfigError]) -> String {
	errors
		.iter()
		.map(ToString::to_string)
		.collect::<Vec<_>>()
		.join("\n")
}

/// One configuration error, with everything needed to fix it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigError {
	/// The file that contains the mistake.
	pub file: RepoPath,
	/// Where in `file` the mistake is, when it is known.
	pub position: Option<Position>,
	/// The setting that contains the mistake, such as `documents.ignore[1]`,
	/// when it is known.
	pub setting: Option<String>,
	/// When `file` is a preset that is wrong only because of where it is
	/// imported: the config that imports it, then every preset on the way to
	/// `file`, outermost first. Empty otherwise.
	pub import_chain: Vec<RepoPath>,
	/// What is wrong.
	pub kind: ConfigErrorKind,
	/// A concrete correction, such as "Use '*.md'".
	pub correction: Option<String>,
}

impl ConfigError {
	/// An error of `kind` in `file`, without a position, setting, chain or
	/// correction.
	pub(super) fn new(file: &RepoPath, kind: ConfigErrorKind) -> Self {
		Self {
			file: file.clone(),
			position: None,
			setting: None,
			import_chain: Vec::new(),
			kind,
			correction: None,
		}
	}

	/// Sets the position.
	pub(super) fn at(mut self, position: Option<Position>) -> Self {
		self.position = position;
		self
	}

	/// Sets the setting's name.
	pub(super) fn in_setting(mut self, setting: impl Into<String>) -> Self {
		self.setting = Some(setting.into());
		self
	}

	/// Sets the correction.
	pub(super) fn correct(mut self, correction: impl Into<String>) -> Self {
		self.correction = Some(correction.into());
		self
	}

	/// Sets the import chain.
	pub(super) fn through(mut self, chain: Vec<RepoPath>) -> Self {
		self.import_chain = chain;
		self
	}
}

impl fmt::Display for ConfigError {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(formatter, "{}", self.file)?;
		if let Some(position) = self.position {
			write!(formatter, ":{position}")?;
		}
		formatter.write_str(": ")?;
		if let Some(setting) = &self.setting {
			write!(formatter, "`{setting}`: ")?;
		}
		write!(formatter, "{}", self.kind)?;
		if !self.import_chain.is_empty() {
			let chain = self
				.import_chain
				.iter()
				.map(ToString::to_string)
				.collect::<Vec<_>>()
				.join(" → ");
			write!(formatter, " (imported through {chain})")?;
		}
		if let Some(correction) = &self.correction {
			write!(formatter, ". {correction}")?;
		}
		Ok(())
	}
}

impl std::error::Error for ConfigError {}

/// What is wrong with a config.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ConfigErrorKind {
	/// The file is not UTF-8.
	#[error("the file is not valid UTF-8")]
	NotUtf8,
	/// A YAML or type error with no more specific kind.
	#[error("{message}")]
	Yaml {
		/// The parser's description.
		message: String,
	},
	/// An unquoted value starting with `*`, such as a glob, which YAML reads as
	/// an alias.
	#[error("YAML read `{token}` as an alias, because it starts with `*`")]
	UnquotedAlias {
		/// The value as written.
		token: String,
	},
	/// An unquoted value starting with `!`, such as an ignore exception, which
	/// YAML reads as a tag.
	#[error("YAML read `{tag}` as a tag, because it starts with `!`")]
	UnsupportedTag {
		/// The tag as YAML read it.
		tag: String,
	},
	/// A YAML 1.1 boolean, such as `no`.
	#[error("`{value}` is not a boolean; only `true` and `false` are")]
	NonStrictBoolean {
		/// The value as written.
		value: String,
	},
	/// A float, string or negative number where a whole number belongs.
	#[error("`{value}` is not a whole number")]
	NotWholeNumber {
		/// The value as written.
		value: String,
	},
	/// The same key twice in one mapping.
	#[error("`{key}` appears twice in the same mapping")]
	DuplicateKey {
		/// The repeated key.
		key: String,
	},
	/// A merge key, `<<`.
	#[error("merge keys (`<<`) are not supported")]
	MergeKey,
	/// More than one YAML document in a file.
	#[error("the file holds more than one YAML document")]
	MultipleDocuments,
	/// A key that no config allows.
	#[error("unknown setting `{key}`")]
	UnknownSetting {
		/// The key as written.
		key: String,
	},
	/// A key that other kinds of config allow, but this one does not.
	#[error("a {kind} cannot set `{key}`")]
	ForbiddenSetting {
		/// The key.
		key: String,
		/// The kind of config whose rules it breaks.
		kind: ConfigKind,
	},
	/// A required key is missing.
	#[error("`{key}` is required")]
	MissingSetting {
		/// The missing key.
		key: String,
	},
	/// A key with an empty value, which would reset nothing because settings
	/// inherit one by one.
	#[error("the value is empty")]
	EmptyValue,
	/// A `version` that this build does not support.
	#[error("version {version} is not supported")]
	UnsupportedVersion {
		/// The version as written.
		version: u32,
	},
	/// A name that breaks the naming rules.
	#[error("`{name}` is not a valid {what}")]
	InvalidName {
		/// The name as written.
		name: String,
		/// What the name names, such as "context name".
		what: &'static str,
	},
	/// An import identifier that cannot name a preset.
	#[error("`{name}` is not a valid preset name: {reason}")]
	InvalidImport {
		/// The identifier as written.
		name: String,
		/// Why it was rejected.
		reason: String,
	},
	/// An import of a preset that does not exist.
	#[error("no preset named `{name}` exists in .source-sentry/presets/")]
	UnknownPreset {
		/// The identifier as written.
		name: String,
	},
	/// Presets that import each other in a cycle.
	#[error("the imports form a cycle: {}", .cycle.join(" → "))]
	ImportCycle {
		/// The presets on the cycle, starting and ending with the same one.
		cycle: Vec<String>,
	},
	/// A class that the repository config does not declare.
	#[error("class `{class}` is not declared")]
	UndeclaredClass {
		/// The class as written.
		class: String,
	},
	/// A context that the repository config does not declare.
	#[error("context `{context}` is not declared")]
	UndeclaredContext {
		/// The context as written.
		context: String,
	},
	/// A rule that targets `fresh`, which always passes.
	#[error("no rule may target `fresh`, which always passes")]
	FreshRule,
	/// A configured cadence that is not a valid recurrence rule, which is
	/// reported whether or not a document uses it.
	#[error("{reason}")]
	InvalidSchedule {
		/// Why.
		reason: String,
	},
	/// A rule with an empty `facts` list, which matches nothing.
	#[error("a rule's `facts` list is empty, so the rule matches nothing")]
	NoFacts,
	/// A path pattern that cannot be compiled.
	#[error("`{pattern}` is not a valid path pattern: {reason}")]
	InvalidPattern {
		/// The pattern as written.
		pattern: String,
		/// Why it was rejected.
		reason: String,
	},
	/// A series pattern that cannot be compiled, or has no `version` group.
	#[error("`{pattern}` is not a valid series pattern: {reason}")]
	InvalidSeriesPattern {
		/// The pattern as written.
		pattern: String,
		/// Why it was rejected.
		reason: String,
	},
}
