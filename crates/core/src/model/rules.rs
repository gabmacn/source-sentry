//! Facts, outcomes, classes and contexts.

use std::fmt;

use serde::{Deserialize, Serialize};

/// One independent property of a document.
#[derive(
	Clone,
	Copy,
	Debug,
	PartialEq,
	Eq,
	PartialOrd,
	Ord,
	Hash,
	Serialize,
	Deserialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum Fact {
	/// Every source matches its attestation. Always passes, and no rule may
	/// target it.
	Fresh,
	/// A source differs from its attestation.
	Stale,
	/// A deadline has arrived: from the start of the due day, or from the
	/// start of the clock when the document is due at once. An overdue
	/// document is also due.
	Due,
	/// A deadline has passed.
	Overdue,
	/// Declared, but never reviewed.
	Unattested,
	/// Inside a root, eligible as a document, and claimed by nothing.
	Unclaimed,
	/// A source that is itself a document is stale, unattested or overdue.
	UpstreamStale,
	/// A source that is itself a document is due.
	UpstreamDue,
	/// Drift exists, but its start cannot be dated.
	TimingUnknown,
	/// A closed series window has no entry.
	Missed,
	/// A series successor exists, and this predecessor is not sealed yet.
	AwaitingSeal,
}

impl Fact {
	/// The name used in configs and reports.
	#[must_use]
	pub fn name(self) -> &'static str {
		match self {
			Self::Fresh => "fresh",
			Self::Stale => "stale",
			Self::Due => "due",
			Self::Overdue => "overdue",
			Self::Unattested => "unattested",
			Self::Unclaimed => "unclaimed",
			Self::UpstreamStale => "upstream-stale",
			Self::UpstreamDue => "upstream-due",
			Self::TimingUnknown => "timing-unknown",
			Self::Missed => "missed",
			Self::AwaitingSeal => "awaiting-seal",
		}
	}
}

impl fmt::Display for Fact {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		formatter.write_str(self.name())
	}
}

/// What a fact means for a build, ordered from least to most severe.
#[derive(
	Clone,
	Copy,
	Debug,
	PartialEq,
	Eq,
	PartialOrd,
	Ord,
	Hash,
	Serialize,
	Deserialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum Outcome {
	/// Nothing to report.
	Pass,
	/// Listed in the report, but never fails either gate.
	Notice,
	/// Fails the report gate only.
	Warn,
	/// Fails both gates.
	Fail,
}

impl Outcome {
	/// The name used in configs and reports.
	#[must_use]
	pub fn name(self) -> &'static str {
		match self {
			Self::Pass => "pass",
			Self::Notice => "notice",
			Self::Warn => "warn",
			Self::Fail => "fail",
		}
	}
}

impl fmt::Display for Outcome {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		formatter.write_str(self.name())
	}
}

/// The name of a class, which the repository config must declare.
#[derive(
	Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(try_from = "String", into = "String")]
pub struct ClassName(String);

/// Why a class name was rejected.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("`{0}` is not a valid class name; use lowercase kebab-case")]
pub struct ClassNameError(String);

impl ClassName {
	/// Validates a class name: lowercase ASCII letters and digits, separated
	/// by single hyphens.
	///
	/// # Errors
	///
	/// Fails when the name is not lowercase kebab-case.
	pub fn new(name: &str) -> Result<Self, ClassNameError> {
		let valid = !name.is_empty()
			&& name.split('-').all(|part| {
				!part.is_empty()
					&& part
						.bytes()
						.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
			});
		if valid {
			Ok(Self(name.to_owned()))
		} else {
			Err(ClassNameError(name.to_owned()))
		}
	}

	/// The name as written.
	#[must_use]
	pub fn as_str(&self) -> &str {
		&self.0
	}
}

impl fmt::Display for ClassName {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		formatter.write_str(&self.0)
	}
}

impl TryFrom<String> for ClassName {
	type Error = ClassNameError;

	fn try_from(name: String) -> Result<Self, Self::Error> {
		Self::new(&name)
	}
}

impl From<ClassName> for String {
	fn from(name: ClassName) -> Self {
		name.0
	}
}

/// A context: what each fact means for a build.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Context {
	/// The outcome for any fact that no rule matches.
	pub fallback: Outcome,
	/// The context's rules.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub rules: Vec<Rule>,
}

/// One rule of a context.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Rule {
	/// The classes the rule applies to. Empty means any class.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub classes: Vec<ClassName>,
	/// The facts the rule applies to.
	pub facts: Vec<Fact>,
	/// The outcome when the rule matches.
	pub outcome: Outcome,
}
