//! Writes a declaration as a `source-sentry` block in the house style of
//! `structure/example.yaml`.
//!
//! - Fields come in the example's order: `reviewed`, `completed`, `classes`,
//!   `owners`, `schedule` (`grace`, then `cadence`) and `sources`; a path
//!   source lists `path`, `oid` and `reviewed`, and a pattern source lists
//!   `pattern`, `reviewed` and `members`. `completed`, which the example lacks,
//!   follows `reviewed`, the other review event.
//! - Indentation is two spaces, and sequences of mappings are indented under
//!   their key.
//! - `classes` and `owners` are flow sequences.
//! - Every string is single-quoted, as in the example, unless it holds a
//!   character that single quotes cannot carry; it is then double-quoted
//!   with escapes. Numbers, dates, timestamps and `null` are plain.
//! - Timestamps are RFC 3339 in UTC, with fractional seconds only when the
//!   value has them.
//! - Sources and members keep the order they are given in.

use std::fmt::Write as _;

use chrono::{DateTime, SecondsFormat, Utc};

use crate::model::{
	CadenceSetting, ClassName, Declaration, Review, Schedule, SourceDecl,
};

use super::scan::Eol;

/// The block for `declaration`, ending with a line ending.
pub(super) fn block(declaration: &Declaration, eol: Eol) -> String {
	let mut out = Out {
		text: String::new(),
		eol: eol.as_str(),
	};
	if *declaration == Declaration::default() {
		out.line(0, "source-sentry: {}");
		return out.text;
	}
	out.line(0, "source-sentry:");
	if let Some(review) = &declaration.reviewed {
		out.review(1, "reviewed", review);
	}
	if let Some(review) = &declaration.completed {
		out.review(1, "completed", review);
	}
	if !declaration.classes.is_empty() {
		let classes = declaration.classes.iter().map(ClassName::as_str);
		out.line(1, &format!("classes: {}", flow(classes)));
	}
	if !declaration.owners.is_empty() {
		let owners = declaration.owners.iter().map(String::as_str);
		out.line(1, &format!("owners: {}", flow(owners)));
	}
	if let Some(schedule) = &declaration.schedule {
		out.schedule(schedule);
	}
	if !declaration.sources.is_empty() {
		out.line(1, "sources:");
		for source in &declaration.sources {
			out.source(source);
		}
	}
	out.text
}

/// The text being written.
struct Out<'a> {
	text: String,
	eol: &'a str,
}

impl Out<'_> {
	/// Writes `content` on its own line, `depth` levels deep.
	fn line(&mut self, depth: usize, content: &str) {
		for _ in 0..depth {
			self.text.push_str("  ");
		}
		self.text.push_str(content);
		self.text.push_str(self.eol);
	}

	fn review(&mut self, depth: usize, key: &str, review: &Review) {
		self.line(depth, &format!("{key}:"));
		self.line(depth + 1, &format!("at: {}", timestamp(&review.at)));
		let by = review
			.by
			.as_deref()
			.map_or_else(|| "null".to_owned(), quote);
		self.line(depth + 1, &format!("by: {by}"));
	}

	fn schedule(&mut self, schedule: &Schedule) {
		if schedule.grace.is_none() && schedule.cadence.is_inherit() {
			self.line(1, "schedule: {}");
			return;
		}
		self.line(1, "schedule:");
		if let Some(grace) = schedule.grace {
			self.line(2, &format!("grace: {grace}"));
		}
		match &schedule.cadence {
			CadenceSetting::Inherit => {}
			// Removes an inherited cadence.
			CadenceSetting::Remove => self.line(2, "cadence: null"),
			CadenceSetting::Set(cadence) => {
				self.line(2, "cadence:");
				self.line(
					3,
					&format!("dtstart: {}", cadence.dtstart.format("%Y-%m-%d")),
				);
				self.line(3, &format!("rrule: {}", quote(&cadence.rrule)));
			}
		}
	}

	fn source(&mut self, source: &SourceDecl) {
		match source {
			SourceDecl::Path(source) => {
				self.line(2, &format!("- path: {}", quote(&source.path)));
				if let Some(oid) = &source.oid {
					self.line(3, &format!("oid: {}", quote(&oid.to_string())));
				}
				if let Some(review) = &source.reviewed {
					self.review(3, "reviewed", review);
				}
			}
			SourceDecl::Pattern(source) => {
				self.line(2, &format!("- pattern: {}", quote(&source.pattern)));
				if let Some(review) = &source.reviewed {
					self.review(3, "reviewed", review);
				}
				if !source.members.is_empty() {
					self.line(3, "members:");
				}
				for member in &source.members {
					self.line(4, &format!("- path: {}", quote(&member.path)));
					self.line(5, &format!("oid: {}", quote(&member.oid.to_string())));
					self.review(5, "reviewed", &member.reviewed);
				}
			}
		}
	}
}

/// A flow sequence of quoted strings, such as `['a', 'b']`.
fn flow<'a>(items: impl Iterator<Item = &'a str>) -> String {
	let items: Vec<String> = items.map(quote).collect();
	format!("[{}]", items.join(", "))
}

/// An RFC 3339 timestamp in UTC, such as `2026-11-07T15:02:44Z`.
fn timestamp(at: &DateTime<Utc>) -> String {
	at.to_rfc3339_opts(SecondsFormat::AutoSi, true)
}

/// `value` as a quoted YAML scalar that reads back as exactly `value`.
pub(super) fn quote(value: &str) -> String {
	if value.chars().any(needs_escape) {
		double_quoted(value)
	} else {
		format!("'{}'", value.replace('\'', "''"))
	}
}

/// Whether a single-quoted scalar cannot carry `character` as written: line
/// breaks would be folded, and YAML forbids other control characters,
/// non-characters and a byte order mark inside a document. Tabs are escaped
/// too, so that they stay visible.
fn needs_escape(character: char) -> bool {
	character.is_control()
		|| matches!(
			character,
			'\u{2028}' | '\u{2029}' | '\u{FEFF}' | '\u{FFFE}' | '\u{FFFF}'
		)
}

/// `value` as a double-quoted scalar, escaping what [`needs_escape`] flags.
fn double_quoted(value: &str) -> String {
	let mut quoted = String::from("\"");
	for character in value.chars() {
		match character {
			'"' => quoted.push_str("\\\""),
			'\\' => quoted.push_str("\\\\"),
			'\n' => quoted.push_str("\\n"),
			'\t' => quoted.push_str("\\t"),
			character if needs_escape(character) => {
				let code = u32::from(character);
				// Writing to a `String` cannot fail.
				let _ = if code <= 0xFF {
					write!(quoted, "\\x{code:02X}")
				} else {
					write!(quoted, "\\u{code:04X}")
				};
			}
			character => quoted.push(character),
		}
	}
	quoted.push('"');
	quoted
}
