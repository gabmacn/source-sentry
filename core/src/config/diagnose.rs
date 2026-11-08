//! Strict YAML options, and the translation of serde-saphyr's errors into
//! configuration errors that name the setting and suggest a correction.

use std::fmt::Write as _;

use serde::de::DeserializeOwned;
use serde_path_to_error::{Path, Segment};
use serde_saphyr::{Error, MessageFormatter, UserMessageFormatter};

use super::ConfigKind;
use super::error::{ConfigError, ConfigErrorKind, Position};
use super::parse::{WHOLE_NUMBER, position_of};
use super::pattern::quote;
use crate::model::RepoPath;

/// The key under which serde-saphyr's `Spanned` exposes the wrapped value.
const SPANNED_VALUE: &str = "value";

/// The strict parser options. Merge keys, duplicate keys, unsupported tags and
/// YAML 1.1 booleans are rejected; the single-document entry point rejects a
/// second document.
fn options() -> serde_saphyr::Options {
	crate::yaml::strict_options()
}

/// Deserializes `text`, the contents of `file`, strictly.
///
/// # Errors
///
/// Returns the first YAML or type error, with the setting it occurred in,
/// its position, and a correction where one is known. It is boxed because it
/// is large and rare.
pub(super) fn deserialize<T: DeserializeOwned>(
	file: &RepoPath,
	kind: ConfigKind,
	text: &str,
) -> Result<T, Box<ConfigError>> {
	let mut setting = None;
	let result = serde_saphyr::with_deserializer_from_str_with_options(
		text,
		options(),
		|deserializer| {
			serde_path_to_error::deserialize(deserializer).map_err(|error| {
				setting = render_setting(error.path());
				error.into_inner()
			})
		},
	);
	result.map_err(|error| Box::new(diagnose(file, kind, text, &error, setting)))
}

/// Renders a path such as `documents.ignore[1]`.
///
/// serde-saphyr's `Spanned` reads its value through a mapping key named
/// `value`, which appears in the path after every spanned position. In the
/// raw config shapes, every position is spanned until a model type is
/// reached (see `parse`), so a segment is followed by that synthetic key
/// until the first segment that is not; from there on, the path is the model
/// type's own.
fn render_setting(path: &Path) -> Option<String> {
	let mut rendered = String::new();
	let mut segments = path.iter().peekable();
	let mut spanned = true;
	while let Some(segment) = segments.next() {
		match segment {
			Segment::Seq { index } => {
				// Writing to a `String` cannot fail.
				let _ = write!(rendered, "[{index}]");
			}
			Segment::Map { key } | Segment::Enum { variant: key } => {
				if !rendered.is_empty() {
					rendered.push('.');
				}
				rendered.push_str(key);
			}
			Segment::Unknown => continue,
		}
		if spanned {
			match segments.peek() {
				Some(Segment::Map { key }) if key == SPANNED_VALUE => {
					segments.next();
				}
				Some(_) => spanned = false,
				None => {}
			}
		}
	}
	(!rendered.is_empty()).then_some(rendered)
}

/// Translates a serde-saphyr error.
fn diagnose(
	file: &RepoPath,
	kind: ConfigKind,
	text: &str,
	error: &Error,
	setting: Option<String>,
) -> ConfigError {
	let position = error
		.location()
		.filter(|location| location.line() > 0)
		.map(|location| position_of(&location));
	let token = position.and_then(|position| scalar_at(text, position));
	let (error_kind, correction) = classify(
		error.without_snippet(),
		kind,
		text,
		position,
		token,
		setting.as_deref(),
	);
	let mut diagnosed = ConfigError::new(file, error_kind).at(position);
	diagnosed.setting = setting;
	diagnosed.correction = correction;
	diagnosed
}

/// The error's kind and correction.
fn classify(
	error: &Error,
	kind: ConfigKind,
	text: &str,
	position: Option<Position>,
	token: Option<String>,
	setting: Option<&str>,
) -> (ConfigErrorKind, Option<String>) {
	match error {
		Error::UnknownAnchor { .. } => {
			let token = token.unwrap_or_else(|| "*".to_owned());
			let correction = format!("Quote it: use {}", quote(&token));
			(ConfigErrorKind::UnquotedAlias { token }, Some(correction))
		}
		Error::UnsupportedTag { tag, .. } => {
			let scalar = position
				.and_then(|position| tagged_scalar(text, position, tag))
				.unwrap_or_else(|| tag.clone());
			(
				ConfigErrorKind::UnsupportedTag { tag: tag.clone() },
				Some(format!(
					"If the `!` belongs to the value, quote it: use {}",
					quote(&scalar)
				)),
			)
		}
		Error::InvalidBooleanStrict { .. } => {
			let value = token.unwrap_or_default();
			let correction = match value.to_ascii_lowercase().as_str() {
				"yes" | "y" | "on" | "true" => "Use `true`",
				"no" | "n" | "off" | "false" => "Use `false`",
				_ => "Use `true` or `false`",
			};
			(
				ConfigErrorKind::NonStrictBoolean { value },
				Some(correction.to_owned()),
			)
		}
		Error::DuplicateMappingKey { key, .. } => (
			ConfigErrorKind::DuplicateKey {
				key: key.clone().unwrap_or_default(),
			},
			Some("Keep only one of them".to_owned()),
		),
		Error::MergeKeyNotAllowed { .. } => (
			ConfigErrorKind::MergeKey,
			Some(
				"Write the settings out, or move shared settings into a preset and \
				 list it under `imports`"
					.to_owned(),
			),
		),
		Error::MultipleDocuments { .. } => (
			ConfigErrorKind::MultipleDocuments,
			Some(
				"Remove the `---` line and everything after it; a config holds one \
				 document"
					.to_owned(),
			),
		),
		Error::SerdeUnknownField {
			field, expected, ..
		} => {
			// The setting ends with the unknown key; what precedes it is the
			// mapping that does not allow it.
			let parent = setting
				.and_then(|setting| setting.strip_suffix(field.as_str()))
				.map(|parent| parent.trim_end_matches('.'))
				.filter(|parent| !parent.is_empty());
			let correction = match parent {
				Some(parent) => format!("`{parent}` allows {}", list(expected)),
				None => format!("A {kind} allows {}", list(&kind.allowed_keys())),
			};
			(
				ConfigErrorKind::UnknownSetting { key: field.clone() },
				Some(correction),
			)
		}
		Error::SerdeMissingField { field, .. } => (
			ConfigErrorKind::MissingSetting {
				key: (*field).to_owned(),
			},
			Some(format!("Add `{field}`")),
		),
		Error::SerdeUnknownVariant {
			variant, expected, ..
		} => (
			ConfigErrorKind::Yaml {
				message: format!("unknown value `{variant}`"),
			},
			Some(format!("Use {}", list(expected))),
		),
		Error::InvalidScalar { ty, .. } if is_integer_type(ty) => {
			not_whole_number(token, setting)
		}
		Error::SerdeInvalidType { expected, .. }
		| Error::SerdeInvalidValue { expected, .. }
			if expected == WHOLE_NUMBER =>
		{
			not_whole_number(token, setting)
		}
		_ => {
			let message = UserMessageFormatter.format_message(error).into_owned();
			let correction = (setting == Some("time-zone")).then(|| {
				"Use an IANA name in quotes, such as 'Europe/Madrid'".to_owned()
			});
			(ConfigErrorKind::Yaml { message }, correction)
		}
	}
}

/// Whether `ty`, a Rust type name from serde-saphyr, is an integer type.
fn is_integer_type(ty: &str) -> bool {
	ty.starts_with('u') || ty.starts_with('i')
}

/// A whole number was written as something else, such as `1.10`, which a float
/// would read as `1.1`.
fn not_whole_number(
	token: Option<String>,
	setting: Option<&str>,
) -> (ConfigErrorKind, Option<String>) {
	let correction = if setting == Some("version") {
		"Version identifiers are integers, never floats or strings: use `version: 1`"
	} else {
		"Use a whole number, such as `7`, without quotes or decimals"
	};
	(
		ConfigErrorKind::NotWholeNumber {
			value: token.unwrap_or_default(),
		},
		Some(correction.to_owned()),
	)
}

/// Lists names for a correction, such as "`a`, `b` or `c`".
fn list(names: &[&str]) -> String {
	let quoted: Vec<String> =
		names.iter().map(|name| format!("`{name}`")).collect();
	match quoted.split_last() {
		None => "nothing".to_owned(),
		Some((last, [])) => last.clone(),
		Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
	}
}

/// The line that `position` is on, and its characters from the column on.
fn rest_of_line(text: &str, position: Position) -> Option<(&str, String)> {
	let line_index = usize::try_from(position.line).ok()?.checked_sub(1)?;
	let column = usize::try_from(position.column).ok()?.checked_sub(1)?;
	let line = text.lines().nth(line_index)?;
	Some((line, line.chars().skip(column).collect()))
}

/// The plain scalar at `position`, as written. When `position` points at a
/// key, which serde reports for some type errors, the key's value is taken
/// instead.
fn scalar_at(text: &str, position: Position) -> Option<String> {
	let (_, rest) = rest_of_line(text, position)?;
	let rest = match rest.split_once(':') {
		Some((key, value))
			if !key.is_empty()
				&& key.chars().all(|character| {
					character.is_ascii_alphanumeric() || character == '-'
				}) =>
		{
			value.to_owned()
		}
		_ => rest,
	};
	let scalar = plain_scalar(&rest, true);
	(!scalar.is_empty()).then_some(scalar)
}

/// The value that an unsupported tag was read from, starting at the tag: the
/// tag and the scalar that follows it on the same line.
fn tagged_scalar(text: &str, position: Position, tag: &str) -> Option<String> {
	let (line, _) = rest_of_line(text, position)?;
	let start = line.find(tag)?;
	let in_flow = line[..start].contains(['[', '{']);
	let scalar = plain_scalar(&line[start..], in_flow);
	(!scalar.is_empty()).then_some(scalar)
}

/// A plain scalar at the start of `text`: up to a comment, or, inside a flow
/// collection, up to the next `,`, `]` or `}`.
fn plain_scalar(text: &str, in_flow: bool) -> String {
	let mut end = text.len();
	if let Some(comment) = text.find(" #") {
		end = end.min(comment);
	}
	if in_flow && let Some(flow) = text.find([',', ']', '}']) {
		end = end.min(flow);
	}
	text[..end].trim().to_owned()
}
