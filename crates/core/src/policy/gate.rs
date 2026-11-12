//! Gates and exit codes.

use std::{fmt, str::FromStr};

use crate::model::Outcome;

use super::ErrorKind;

/// The exit code for invalid command-line usage. The CLI owns it;
/// it is listed here so that every code is defined in one place.
pub const USAGE_EXIT_CODE: u8 = 64;

/// Which outcome makes the exit non-zero, set by `--fail-on`.
///
/// The report gate runs with [`Threshold::Warn`] and the continue gate with
/// [`Threshold::Fail`]. Neither threshold lets a notice fail.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Threshold {
	/// A warning or a failure exits non-zero. The default.
	#[default]
	Warn,
	/// Only a failure exits non-zero.
	Fail,
}

impl Threshold {
	/// The value as written after `--fail-on`.
	#[must_use]
	pub fn name(self) -> &'static str {
		match self {
			Self::Warn => "warn",
			Self::Fail => "fail",
		}
	}
}

impl fmt::Display for Threshold {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		formatter.write_str(self.name())
	}
}

/// A `--fail-on` value that is neither `warn` nor `fail`.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("`{0}` is not a threshold; use `warn` or `fail`")]
pub struct ThresholdError(String);

impl FromStr for Threshold {
	type Err = ThresholdError;

	/// Parses `warn` or `fail`, exactly as written.
	fn from_str(value: &str) -> Result<Self, Self::Err> {
		match value {
			"warn" => Ok(Self::Warn),
			"fail" => Ok(Self::Fail),
			_ => Err(ThresholdError(value.to_owned())),
		}
	}
}

/// The exit code of a check. Variants are ordered by code, so the
/// greater of two is the more urgent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ExitCode {
	/// 0: nothing reached the threshold. Notices always end here.
	BelowThreshold,
	/// 1: a warning, with the threshold at `warn`.
	Warning,
	/// 2: a document failed.
	Failure,
	/// 3: a configuration or integrity error.
	ConfigurationOrIntegrity,
	/// 4: an operational error; the evaluation could not complete.
	Operational,
}

impl ExitCode {
	/// The process exit code.
	#[must_use]
	pub fn code(self) -> u8 {
		match self {
			Self::BelowThreshold => 0,
			Self::Warning => 1,
			Self::Failure => 2,
			Self::ConfigurationOrIntegrity => 3,
			Self::Operational => 4,
		}
	}

	/// Whether the gate passes, which is exit 0.
	#[must_use]
	pub fn passes(self) -> bool {
		self == Self::BelowThreshold
	}
}

/// What an evaluation found: each document's outcome, and the errors, counted
/// apart.
///
/// Errors are recorded only through [`Summary::record_error`], never as an
/// [`Outcome`], and [`Summary::exit_code`] ranks any error above any outcome.
/// Contexts produce outcomes only, so they cannot downgrade an error.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Summary {
	passes: usize,
	notices: usize,
	warnings: usize,
	failures: usize,
	configuration_errors: usize,
	integrity_errors: usize,
	operational_errors: usize,
}

impl Summary {
	/// An empty summary: no documents and no errors.
	#[must_use]
	pub fn new() -> Self {
		Self::default()
	}

	/// Records one document's outcome, usually a
	/// [`Judgement`](super::Judgement)'s.
	pub fn record_outcome(&mut self, outcome: Outcome) {
		let count = match outcome {
			Outcome::Pass => &mut self.passes,
			Outcome::Notice => &mut self.notices,
			Outcome::Warn => &mut self.warnings,
			Outcome::Fail => &mut self.failures,
		};
		*count = count.saturating_add(1);
	}

	/// Records one error.
	pub fn record_error(&mut self, kind: ErrorKind) {
		let count = match kind {
			ErrorKind::Configuration => &mut self.configuration_errors,
			ErrorKind::Integrity => &mut self.integrity_errors,
			ErrorKind::Operational => &mut self.operational_errors,
		};
		*count = count.saturating_add(1);
	}

	/// How many documents had `outcome`.
	#[must_use]
	pub fn outcomes(&self, outcome: Outcome) -> usize {
		match outcome {
			Outcome::Pass => self.passes,
			Outcome::Notice => self.notices,
			Outcome::Warn => self.warnings,
			Outcome::Fail => self.failures,
		}
	}

	/// How many errors of `kind` were recorded.
	#[must_use]
	pub fn errors(&self, kind: ErrorKind) -> usize {
		match kind {
			ErrorKind::Configuration => self.configuration_errors,
			ErrorKind::Integrity => self.integrity_errors,
			ErrorKind::Operational => self.operational_errors,
		}
	}

	/// The exit code under `threshold`:
	///
	/// 1. 4 if any operational error was recorded;
	/// 2. otherwise 3 if any configuration or integrity error was;
	/// 3. otherwise 2 if any document failed;
	/// 4. otherwise 1 if any document warned and `threshold` is `warn`;
	/// 5. otherwise 0. A notice always exits 0.
	#[must_use]
	pub fn exit_code(&self, threshold: Threshold) -> ExitCode {
		if self.operational_errors > 0 {
			ExitCode::Operational
		} else if self.configuration_errors > 0 || self.integrity_errors > 0 {
			ExitCode::ConfigurationOrIntegrity
		} else if self.failures > 0 {
			ExitCode::Failure
		} else if self.warnings > 0 && threshold == Threshold::Warn {
			ExitCode::Warning
		} else {
			ExitCode::BelowThreshold
		}
	}
}
