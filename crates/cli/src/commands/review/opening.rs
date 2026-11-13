//! The opening of a session: the facts of the review ahead, which a terminal
//! shows beside the mark.

use std::fmt::Write as _;

use source_sentry_core::model::Outcome;

use crate::{mark, output};

/// The column where the title and the facts start, clear of the mark.
const COLUMN: usize = 28;
/// The line of the mark that carries the title.
const TITLE_LINE: usize = 2;
/// The line of the mark that carries the first fact; the others follow it.
const FIRST_FACT_LINE: usize = 4;
/// The width of the facts' label column.
const LABEL_WIDTH: usize = 11;

/// What a reviewer should know before the first question.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Opening {
	/// The name every recorded review will carry, when one is configured.
	pub(super) reviewer: Option<String>,
	/// The context that decides the queue's order and outcomes.
	pub(super) context: String,
	/// How many projects the session traverses.
	pub(super) projects: usize,
	/// How many documents wait in the queue.
	pub(super) queued: usize,
	/// How many documents the queue was drawn from: the named documents, or
	/// else every tracked one.
	pub(super) tracked: usize,
	/// How many queued documents have each outcome, worst first.
	pub(super) outcomes: Vec<(Outcome, usize)>,
}

impl Opening {
	/// The mark with the product, its version and the facts beside it,
	/// without a final newline.
	pub(super) fn text(&self) -> String {
		let facts = [
			(
				"Reviewer",
				self
					.reviewer
					.clone()
					.unwrap_or_else(|| "not set".to_owned()),
			),
			("Context", self.context.clone()),
			("Projects", self.projects.to_string()),
			(
				"Documents",
				format!(
					"{} of {} {} review",
					self.queued,
					self.tracked,
					if self.queued == 1 { "needs" } else { "need" }
				),
			),
			(
				"Outcomes",
				self
					.outcomes
					.iter()
					.map(|(outcome, count)| {
						format!("{count} {}", output::outcome_label(*outcome))
					})
					.collect::<Vec<_>>()
					.join(" · "),
			),
		];
		let mut beside = vec![String::new(); mark::MARK.lines().count()];
		if let Some(row) = beside.get_mut(TITLE_LINE) {
			*row =
				output::bold(&format!("Source Sentry {}", env!("CARGO_PKG_VERSION")));
		}
		for (row, (label, value)) in
			beside.iter_mut().skip(FIRST_FACT_LINE).zip(facts)
		{
			*row = format!(
				"{}{value}",
				output::pad(&output::dim(label), label, LABEL_WIDTH)
			);
		}
		let mut text = String::new();
		for (line, row) in mark::MARK.lines().zip(&beside) {
			if !text.is_empty() {
				text.push('\n');
			}
			if row.is_empty() {
				text.push_str(line);
			} else {
				let _ = write!(text, "{line:<COLUMN$}{row}");
			}
		}
		text
	}
}

#[cfg(test)]
mod tests {
	use source_sentry_core::model::Outcome;

	use super::{COLUMN, Opening};
	use crate::mark;

	/// The opening's lines without styles, split at the facts' column.
	fn columns(opening: &Opening) -> Vec<(String, String)> {
		anstream::adapter::strip_str(&opening.text())
			.to_string()
			.lines()
			.map(|line| {
				(
					line.chars().take(COLUMN).collect::<String>(),
					line.chars().skip(COLUMN).collect::<String>(),
				)
			})
			.collect()
	}

	/// The title and the five facts share one column beside an intact mark.
	#[test]
	fn the_facts_line_up_beside_the_mark() {
		let lines = columns(&Opening {
			reviewer: Some("Test Reviewer".to_owned()),
			context: "production".to_owned(),
			projects: 2,
			queued: 5,
			tracked: 28,
			outcomes: vec![
				(Outcome::Fail, 1),
				(Outcome::Warn, 3),
				(Outcome::Notice, 1),
			],
		});
		let beside: Vec<_> = lines.iter().map(|(_, text)| text.as_str()).collect();
		assert_eq!(
			beside,
			[
				"",
				"",
				concat!("Source Sentry ", env!("CARGO_PKG_VERSION")),
				"",
				"Reviewer   Test Reviewer",
				"Context    production",
				"Projects   2",
				"Documents  5 of 28 need review",
				"Outcomes   1 fail · 3 warn · 1 note",
				"",
				"",
				"",
			]
		);
		for ((drawn, _), line) in lines.iter().zip(mark::MARK.lines()) {
			assert_eq!(drawn.trim_end(), line);
		}
	}

	/// One queued document takes the singular, and a missing name is stated.
	#[test]
	fn a_single_document_and_an_unset_reviewer_are_worded_plainly() {
		let lines = columns(&Opening {
			reviewer: None,
			context: "default".to_owned(),
			projects: 1,
			queued: 1,
			tracked: 3,
			outcomes: vec![(Outcome::Warn, 1)],
		});
		assert_eq!(lines[4].1, "Reviewer   not set");
		assert_eq!(lines[7].1, "Documents  1 of 3 needs review");
	}
}
