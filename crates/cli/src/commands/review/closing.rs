//! The closing of a session: what was recorded and what remains. A terminal
//! shows it as the opening's twin, beside the same mark.

use std::fmt::Write as _;

use super::{banner, opening};
use crate::{describe, output};

/// The mark beside each recorded review: a tick, drawn with the square root
/// sign on Windows, whose older console fonts carry no tick.
const TICK: char = if cfg!(windows) { '√' } else { '✓' };

/// What a session did, from its final evaluation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Closing {
	/// The name the reviews carry, when one is configured.
	pub(super) reviewer: Option<String>,
	/// Each recorded review, in order, and whether it was a reread.
	pub(super) recorded: Vec<(String, bool)>,
	/// Each document that still needs review, with its facts.
	pub(super) remaining: Vec<String>,
	/// How long the session lasted, in seconds.
	pub(super) seconds: u64,
	/// The command that runs the final gate.
	pub(super) check: String,
}

impl Closing {
	/// The summary in plain sentences, as `--quiet` prints it.
	pub(super) fn plain(&self) -> String {
		let mut text = format!(
			"Recorded {}.\n",
			describe::count(self.recorded.len(), "review", "reviews")
		);
		for (name, reread) in &self.recorded {
			let _ =
				writeln!(text, "  {name}{}", if *reread { " (reread)" } else { "" });
		}
		let _ = writeln!(text, "{}:", self.still());
		for line in &self.remaining {
			let _ = writeln!(text, "  {line}");
		}
		let _ = write!(text, "Run `{}` for the final gate.", self.check);
		text
	}

	/// The mark with what was done beside it, then each recorded review and
	/// each remaining document, without a final newline.
	pub(super) fn text(&self) -> String {
		let rereads = self.recorded.iter().filter(|(_, reread)| *reread).count();
		let recorded = match (self.recorded.len(), rereads) {
			(0, _) => "no reviews".to_owned(),
			(count, 0) => describe::count(count, "review", "reviews"),
			(count, rereads) => format!(
				"{}, {rereads} with a reread",
				describe::count(count, "review", "reviews")
			),
		};
		let remaining = if self.remaining.is_empty() {
			output::good("none")
		} else {
			describe::count(self.remaining.len(), "document", "documents")
		};
		let mut text = banner::text(&[
			("Reviewer", opening::reviewer(self.reviewer.as_deref())),
			("Recorded", recorded),
			("Remaining", remaining),
			("Took", took(self.seconds)),
		]);
		text.push('\n');
		if !self.recorded.is_empty() {
			text.push('\n');
			let width = self
				.recorded
				.iter()
				.map(|(name, _)| name.chars().count())
				.max()
				.unwrap_or_default();
			let tick = output::good(&TICK.to_string());
			for (name, reread) in &self.recorded {
				let _ = if *reread {
					writeln!(
						text,
						"  {tick} {}   {}",
						output::pad(name, name, width),
						output::dim("reread")
					)
				} else {
					writeln!(text, "  {tick} {name}")
				};
			}
		}
		if !self.remaining.is_empty() {
			let _ = writeln!(text, "\n  {}:", self.still());
			for line in &self.remaining {
				let _ = writeln!(text, "    {line}");
			}
		}
		let _ = write!(text, "\n  Run `{}` for the final gate.", self.check);
		text
	}

	/// How many documents still need review, as a clause.
	fn still(&self) -> String {
		format!(
			"{} still {} review",
			describe::count(self.remaining.len(), "document", "documents"),
			if self.remaining.len() == 1 {
				"needs"
			} else {
				"need"
			}
		)
	}
}

/// A duration in whole minutes, as people say it.
fn took(seconds: u64) -> String {
	let count = |value: u64, singular: &str, plural: &str| {
		describe::count(
			usize::try_from(value).unwrap_or(usize::MAX),
			singular,
			plural,
		)
	};
	match (seconds / 3600, seconds % 3600 / 60) {
		(0, 0) => "under a minute".to_owned(),
		(0, minutes) => count(minutes, "minute", "minutes"),
		(hours, 0) => count(hours, "hour", "hours"),
		(hours, minutes) => format!(
			"{} {}",
			count(hours, "hour", "hours"),
			count(minutes, "minute", "minutes")
		),
	}
}

#[cfg(test)]
mod tests {
	use super::{Closing, TICK, took};
	use crate::commands::review::banner::COLUMN;

	/// A session with work done and work left.
	fn closing() -> Closing {
		Closing {
			reviewer: Some("Test Reviewer".to_owned()),
			recorded: vec![
				("docs/a.md".to_owned(), false),
				("docs/long-name.md".to_owned(), true),
			],
			remaining: vec!["docs/b.md: stale".to_owned()],
			seconds: 14 * 60 + 20,
			check: "source-sentry check".to_owned(),
		}
	}

	/// The plain summary keeps the sentences that scripts and `--quiet` read.
	#[test]
	fn the_plain_summary_states_what_was_recorded_and_what_remains() {
		assert_eq!(
			closing().plain(),
			"Recorded 2 reviews.\n  docs/a.md\n  docs/long-name.md (reread)\n1 document still needs review:\n  docs/b.md: stale\nRun `source-sentry check` for the final gate."
		);
	}

	/// The closing mirrors the opening: the mark with what was done beside
	/// it, then the reviews and what remains.
	#[test]
	fn the_closing_shows_the_session_beside_the_mark() {
		let text = anstream::adapter::strip_str(&closing().text()).to_string();
		let lines: Vec<_> = text.lines().collect();
		let beside: Vec<String> = lines[5..9]
			.iter()
			.map(|line| line.chars().skip(COLUMN).collect())
			.collect();
		assert_eq!(
			beside,
			[
				"Reviewer   Test Reviewer",
				"Recorded   2 reviews, 1 with a reread",
				"Remaining  1 document",
				"Took       14 minutes",
			]
		);
		assert_eq!(
			lines[12..],
			[
				"",
				&format!("  {TICK} docs/a.md"),
				&format!("  {TICK} docs/long-name.md   reread"),
				"",
				"  1 document still needs review:",
				"    docs/b.md: stale",
				"",
				"  Run `source-sentry check` for the final gate.",
			]
		);
	}

	/// Durations are said in whole minutes and hours.
	#[test]
	fn durations_are_said_in_minutes_and_hours() {
		assert_eq!(took(59), "under a minute");
		assert_eq!(took(60), "1 minute");
		assert_eq!(took(3600), "1 hour");
		assert_eq!(took(3600 + 5 * 60 + 59), "1 hour 5 minutes");
	}
}
