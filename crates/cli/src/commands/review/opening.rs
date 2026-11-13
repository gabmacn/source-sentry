//! The opening of a session: the facts of the review ahead, which a terminal
//! shows beside the mark.

use source_sentry_core::model::Outcome;

use super::banner;
use crate::output;

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
	/// The mark with the product, its version, its licence and the facts
	/// beside it, without a final newline.
	pub(super) fn text(&self) -> String {
		banner::text(&[
			("Reviewer", reviewer(self.reviewer.as_deref())),
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
		])
	}
}

/// The reviewer's name, or a statement that none is configured.
pub(super) fn reviewer(name: Option<&str>) -> String {
	name.unwrap_or("not set").to_owned()
}

#[cfg(test)]
mod tests {
	use source_sentry_core::model::Outcome;

	use super::Opening;
	use super::banner::COLUMN;

	/// The opening's text beside the mark, without styles.
	fn beside(opening: &Opening) -> Vec<String> {
		anstream::adapter::strip_str(&opening.text())
			.to_string()
			.lines()
			.map(|line| line.chars().skip(COLUMN).collect())
			.collect()
	}

	/// The five facts follow the title and the licence.
	#[test]
	fn the_facts_follow_the_title() {
		let lines = beside(&Opening {
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
		assert_eq!(
			lines[5..],
			[
				"Reviewer   Test Reviewer",
				"Context    production",
				"Projects   2",
				"Documents  5 of 28 need review",
				"Outcomes   1 fail · 3 warn · 1 note",
				"",
				"",
			]
		);
	}

	/// One queued document takes the singular, and a missing name is stated.
	#[test]
	fn a_single_document_and_an_unset_reviewer_are_worded_plainly() {
		let lines = beside(&Opening {
			reviewer: None,
			context: "default".to_owned(),
			projects: 1,
			queued: 1,
			tracked: 3,
			outcomes: vec![(Outcome::Warn, 1)],
		});
		assert_eq!(lines[5], "Reviewer   not set");
		assert_eq!(lines[8], "Documents  1 of 3 needs review");
	}
}
