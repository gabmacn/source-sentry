//! Series versions, parsed by kind and ordered by value.

use std::cmp::Ordering;
use std::fmt;

use chrono::NaiveDate;

use crate::model::VersionKind;

/// A series member's version, parsed by its series' [`VersionKind`].
///
/// Versions order by value, never as text, so `v0.0.10` follows `v0.0.9`.
/// Equality is equality of value:
///
/// - `integer`: ASCII digits, so `01` and `1` are equal;
/// - `dotted`: ASCII-digit components separated by `.`, compared component
///   by component, with trailing zero components ignored, so `0.3` and
///   `0.3.0` are equal;
/// - `date`: an ISO 8601 calendar date written `YYYY-MM-DD`.
///
/// Members of one series share a kind. Versions of different kinds order by
/// kind first, which only matters if they are ever mixed.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version(Value);

/// The parsed value of a [`Version`].
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Value {
	Integer(Number),
	/// The components, without trailing zero components.
	Dotted(Vec<Number>),
	Date(NaiveDate),
}

/// A non-negative whole number of any size, kept as its decimal digits
/// without leading zeros (`0` for zero), so that no version overflows.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Number(String);

impl Number {
	/// Parses ASCII digits, with any number of leading zeros.
	fn parse(digits: &str) -> Option<Self> {
		if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
			return None;
		}
		let significant = digits.trim_start_matches('0');
		Some(Self(if significant.is_empty() {
			"0".to_owned()
		} else {
			significant.to_owned()
		}))
	}

	fn is_zero(&self) -> bool {
		self.0 == "0"
	}
}

impl Ord for Number {
	/// Without leading zeros, a longer number is larger, and numbers of equal
	/// length order as text.
	fn cmp(&self, other: &Self) -> Ordering {
		self
			.0
			.len()
			.cmp(&other.0.len())
			.then_with(|| self.0.cmp(&other.0))
	}
}

impl PartialOrd for Number {
	fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
		Some(self.cmp(other))
	}
}

/// Why a version did not parse.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("`{text}` is not {}", expected(*.kind))]
pub struct VersionError {
	/// The version as captured.
	pub text: String,
	/// The kind it should have.
	pub kind: VersionKind,
}

/// What a kind of version looks like, for messages.
fn expected(kind: VersionKind) -> &'static str {
	match kind {
		VersionKind::Integer => {
			"an `integer` version: ASCII digits, such as `0001`"
		}
		VersionKind::Dotted => {
			"a `dotted` version: ASCII digits separated by single dots, such as \
			 `0.3.1`"
		}
		VersionKind::Date => {
			"a `date` version: a valid calendar date written `YYYY-MM-DD`"
		}
	}
}

impl Version {
	/// Parses `text`, as a series' `version` group captured it, as a version
	/// of `kind`.
	///
	/// # Errors
	///
	/// Fails when `text` does not have the form of `kind` described on
	/// [`Version`], including a date that does not exist, such as
	/// `2026-02-30`.
	pub fn parse(kind: VersionKind, text: &str) -> Result<Self, VersionError> {
		let value = match kind {
			VersionKind::Integer => Number::parse(text).map(Value::Integer),
			VersionKind::Dotted => parse_dotted(text).map(Value::Dotted),
			VersionKind::Date => parse_date(text).map(Value::Date),
		};
		value.map(Self).ok_or_else(|| VersionError {
			text: text.to_owned(),
			kind,
		})
	}

	/// The version's kind.
	#[must_use]
	pub fn kind(&self) -> VersionKind {
		match self.0 {
			Value::Integer(_) => VersionKind::Integer,
			Value::Dotted(_) => VersionKind::Dotted,
			Value::Date(_) => VersionKind::Date,
		}
	}
}

/// Parses dotted components and drops trailing zero components.
fn parse_dotted(text: &str) -> Option<Vec<Number>> {
	let mut components = text
		.split('.')
		.map(Number::parse)
		.collect::<Option<Vec<_>>>()?;
	while components.last().is_some_and(Number::is_zero) {
		components.pop();
	}
	Some(components)
}

/// Parses exactly `YYYY-MM-DD`, which chrono's `%Y` alone would loosen
/// with signs and longer years.
fn parse_date(text: &str) -> Option<NaiveDate> {
	let bytes = text.as_bytes();
	let shaped = bytes.len() == 10
		&& bytes.iter().enumerate().all(|(index, byte)| {
			if index == 4 || index == 7 {
				*byte == b'-'
			} else {
				byte.is_ascii_digit()
			}
		});
	if !shaped {
		return None;
	}
	NaiveDate::parse_from_str(text, "%Y-%m-%d").ok()
}

impl fmt::Display for Version {
	/// The canonical spelling: no leading zeros, no trailing zero components,
	/// and dates as `YYYY-MM-DD`. The value zero of a `dotted` version is
	/// written `0`.
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		match &self.0 {
			Value::Integer(number) => formatter.write_str(&number.0),
			Value::Dotted(components) if components.is_empty() => {
				formatter.write_str("0")
			}
			Value::Dotted(components) => {
				let text: Vec<&str> = components
					.iter()
					.map(|component| component.0.as_str())
					.collect();
				formatter.write_str(&text.join("."))
			}
			Value::Date(date) => write!(formatter, "{}", date.format("%Y-%m-%d")),
		}
	}
}
