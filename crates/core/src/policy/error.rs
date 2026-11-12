//! Policy errors, and the kinds of error that no context can downgrade.

use crate::model::ClassName;

use super::DEFAULTS_VERSIONS;

/// The kinds of error that stand outside every context.
///
/// An error is not a [`Fact`](crate::model::Fact), so no rule can target it,
/// and it never becomes an [`Outcome`](crate::model::Outcome). A
/// [`Summary`](super::Summary) counts errors apart from outcomes and ranks
/// every error above every outcome, which is what keeps a context from
/// downgrading one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ErrorKind {
	/// The configuration is invalid: for example an unknown context or class,
	/// an unsupported version, or a malformed lockfile or declaration. Exit 3.
	Configuration,
	/// What Source Sentry protects is broken: a broken seal, a missing sealed
	/// record or a double claim. Exit 3.
	Integrity,
	/// The evaluation could not complete, for example because the repository
	/// backend failed or a shallow clone cut the history short. Exit 4.
	Operational,
}

/// A configuration error found while loading, selecting or validating a
/// context.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PolicyError {
	/// The repository config pins a default-policy version that this build
	/// does not ship.
	#[error(
		"default-policy version {version} is not supported; this build supports \
		 {}",
		supported_versions()
	)]
	UnsupportedDefaultsVersion {
		/// The version requested.
		version: u32,
	},
	/// The requested context is not declared. It never
	/// falls back to the default context, so a typo cannot loosen the rules.
	#[error(
		"context `{name}` is not declared in the repository config; choose one \
		 of {}",
		backticked(.available)
	)]
	UnknownContext {
		/// The name requested.
		name: String,
		/// The names that can be selected, sorted.
		available: Vec<String>,
	},
	/// A declared context has invalid rules.
	#[error("context `{name}` is invalid: {}", joined(.errors))]
	InvalidContext {
		/// The context's name.
		name: String,
		/// Every problem found, in rule order.
		errors: Vec<RuleError>,
	},
	/// The built-in default context shipped with this build could not be
	/// read. The crate's tests read every shipped version, so this indicates a
	/// defective build rather than a mistake in the repository.
	#[error(
		"the built-in default context, version {version}, could not be read: \
		 {reason}"
	)]
	BrokenDefaults {
		/// The default-policy version.
		version: u32,
		/// The parser's explanation.
		reason: String,
	},
}

impl PolicyError {
	/// The kind of error, for a [`Summary`](super::Summary). Every policy
	/// error is a configuration error, which exits 3.
	#[must_use]
	pub fn kind(&self) -> ErrorKind {
		match self {
			Self::UnsupportedDefaultsVersion { .. }
			| Self::UnknownContext { .. }
			| Self::InvalidContext { .. }
			| Self::BrokenDefaults { .. } => ErrorKind::Configuration,
		}
	}
}

/// One problem with one rule.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("rule {} {problem}", .index + 1)]
pub struct RuleError {
	/// The rule's position in its list, counted from zero. Messages count from
	/// one, as a person reading the file would.
	pub index: usize,
	/// What is wrong with the rule.
	pub problem: RuleProblem,
}

/// What can be wrong with a rule.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RuleProblem {
	/// The rule lists `fresh`, which always passes and which no rule may
	/// target.
	#[error("targets `fresh`, which always passes; remove it from `facts`")]
	TargetsFresh,
	/// The rule names a class that the repository config does not declare.
	#[error(
		"names the class `{0}`, which `define.classes` in the repository config \
		 does not declare"
	)]
	UndeclaredClass(ClassName),
	/// The rule's `facts` list is empty, so it could never match.
	#[error("has an empty `facts` list; name at least one fact")]
	NoFacts,
}

/// The supported default-policy versions, for messages.
fn supported_versions() -> String {
	let versions: Vec<String> =
		DEFAULTS_VERSIONS.iter().map(u32::to_string).collect();
	versions.join(", ")
}

/// Names in backticks, separated by commas, for messages.
fn backticked(names: &[String]) -> String {
	let names: Vec<String> =
		names.iter().map(|name| format!("`{name}`")).collect();
	names.join(", ")
}

/// Rule errors separated by semicolons, for messages.
fn joined(errors: &[RuleError]) -> String {
	let errors: Vec<String> = errors.iter().map(ToString::to_string).collect();
	errors.join("; ")
}
