//! The closing of a session: what was recorded and what remains. A terminal
//! shows it as the opening's twin, beside the same mark.

use std::fmt::Write as _;
use std::io::{self, Write as _};

use ratatui::crossterm::cursor::{MoveToNextLine, MoveToPreviousLine};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{self, Clear, ClearType};
use ratatui::text::Span;

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

/// A part of the closing. The session prints the first at once, then
/// evaluates the working tree a final time for the second.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Part {
	/// What the session itself knows. Beside the mark, this is the whole
	/// mark, with the count of remaining documents still being checked.
	Known,
	/// What the final evaluation adds: what remains, and the rest.
	Evaluated,
}

/// The row beside the mark that counts the remaining documents, which the
/// final evaluation fills in.
const REMAINING_ROW: usize = banner::FIRST_FACT_LINE + 2;

/// What the remaining row shows until the final evaluation has run.
const CHECKING: &str = "checking…";

impl Closing {
	/// One part of the closing, in plain sentences under `--quiet` and
	/// beside the mark otherwise, without a final newline. The plain parts
	/// together make [`Self::plain`]. Beside the mark, the first part is the
	/// mark with the remaining count still being checked, and the second
	/// follows it; [`Self::remaining_row`] then fills that count in, so that
	/// the whole reads as [`Self::text`].
	pub(super) fn part(&self, part: Part, quiet: bool) -> String {
		match (part, quiet) {
			(Part::Known, false) => self.banner(false),
			(Part::Evaluated, false) => self.rest(),
			(part, true) => {
				// The plain summary's first part is the recorded reviews.
				let whole = self.plain();
				let (head, tail) = split_lines(&whole, 1 + self.recorded.len());
				match part {
					Part::Known => head.to_owned(),
					Part::Evaluated => tail.to_owned(),
				}
			}
		}
	}

	/// The row beside the mark that counts the remaining documents, as the
	/// final evaluation fills it in, and how many lines above the end of the
	/// mark it sits.
	pub(super) fn remaining_row(&self) -> (String, usize) {
		let banner = self.banner(true);
		let row = banner.lines().nth(REMAINING_ROW).unwrap_or_default();
		(row.to_owned(), banner.lines().count() - REMAINING_ROW)
	}

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
		format!("{}\n{}", self.banner(true), self.rest())
	}

	/// The mark with what was done beside it, without a final newline.
	/// Before the final evaluation, the count of remaining documents reads
	/// as still being checked.
	fn banner(&self, evaluated: bool) -> String {
		let rereads = self.recorded.iter().filter(|(_, reread)| *reread).count();
		let recorded = match (self.recorded.len(), rereads) {
			(0, _) => "no reviews".to_owned(),
			(count, 0) => describe::count(count, "review", "reviews"),
			(count, rereads) => format!(
				"{}, {rereads} with a reread",
				describe::count(count, "review", "reviews")
			),
		};
		let remaining = if !evaluated {
			output::dim(CHECKING)
		} else if self.remaining.is_empty() {
			output::good("none")
		} else {
			describe::count(self.remaining.len(), "document", "documents")
		};
		banner::text(&[
			("Reviewer", opening::reviewer(self.reviewer.as_deref())),
			("Recorded", recorded),
			("Remaining", remaining),
			("Took", took(self.seconds)),
		])
	}

	/// What follows the mark: each recorded review, each remaining document
	/// and the final gate, without a final newline.
	fn rest(&self) -> String {
		let mut text = String::new();
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

/// Prints a closing on the terminal in its two parts. Beside the mark, the
/// whole mark comes at once, with the count of remaining documents still
/// being checked; once the final evaluation has run, that count is written
/// over in place and the rest follows. Where the terminal is too narrow to
/// keep each row of the mark on one line, which moving back up relies on,
/// the whole closing waits for the evaluation instead.
pub(super) struct Writer {
	/// Plain sentences without the mark, for `--quiet`.
	quiet: bool,
	/// Whether a blank line parts the closing from what came before it.
	gap: bool,
	/// Whether the mark was printed with the count still being checked.
	checking: bool,
}

/// The columns the remaining count may take beyond what is shown while it
/// is checked, as in "1234 documents".
const ROOM: usize = 8;

impl Writer {
	/// A writer that has printed nothing yet.
	pub(super) fn new(quiet: bool, gap: bool) -> Self {
		Self {
			quiet,
			gap,
			checking: false,
		}
	}

	/// Prints one part of `closing`.
	pub(super) fn show(&mut self, closing: &Closing, part: Part) {
		if self.quiet {
			output::data(&closing.part(part, true));
			return;
		}
		let gap = if self.gap { "\n" } else { "" };
		match part {
			Part::Known => {
				let mark = closing.part(Part::Known, false);
				if fits(&mark) {
					output::data(&format!("{gap}{mark}"));
					self.checking = true;
				}
			}
			Part::Evaluated if self.checking => {
				let (row, up) = closing.remaining_row();
				// Should the cursor not move, the list below still says how
				// many documents remain.
				let _ = overwrite(&row, up);
				output::data(&closing.part(Part::Evaluated, false));
			}
			Part::Evaluated => output::data(&format!("{gap}{}", closing.text())),
		}
	}
}

/// Whether every line of `text` fits the terminal's width on one line,
/// with room for the remaining count to grow.
fn fits(text: &str) -> bool {
	let Ok((columns, _)) = terminal::size() else {
		return false;
	};
	let widest = anstream::adapter::strip_str(text)
		.to_string()
		.lines()
		.map(|line| Span::raw(line).width())
		.max()
		.unwrap_or_default();
	widest + ROOM < usize::from(columns)
}

/// Writes `row` over the line `up` lines above the cursor, then returns to
/// where the cursor was.
fn overwrite(row: &str, up: usize) -> io::Result<()> {
	let up = u16::try_from(up).unwrap_or(u16::MAX);
	execute!(
		io::stdout(),
		MoveToPreviousLine(up),
		Clear(ClearType::CurrentLine)
	)?;
	let mut stdout = anstream::stdout();
	write!(stdout, "{row}")?;
	stdout.flush()?;
	execute!(io::stdout(), MoveToNextLine(up))
}

/// `text` split after its first `lines` lines, without the newline between
/// the two parts.
fn split_lines(text: &str, lines: usize) -> (&str, &str) {
	match text.match_indices('\n').nth(lines.saturating_sub(1)) {
		Some((at, _)) if lines > 0 => (&text[..at], &text[at + 1..]),
		_ if lines == 0 => ("", text),
		_ => (text, ""),
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
	use super::{CHECKING, Closing, Part, REMAINING_ROW, TICK, took};
	use crate::commands::review::banner::COLUMN;
	use crate::{mark, output};

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

	/// The first part is the same whatever remains, so it can be printed
	/// before the final evaluation. In plain sentences the two parts make
	/// the whole. Beside the mark, the first is the whole mark with the
	/// remaining count still being checked, and filling that row in makes
	/// the whole.
	#[test]
	fn the_known_part_does_not_wait_for_what_remains() {
		let before = Closing {
			remaining: Vec::new(),
			..closing()
		};
		for quiet in [false, true] {
			assert_eq!(
				before.part(Part::Known, quiet),
				closing().part(Part::Known, quiet)
			);
		}
		assert_eq!(
			format!(
				"{}\n{}",
				closing().part(Part::Known, true),
				closing().part(Part::Evaluated, true)
			),
			closing().plain()
		);
		assert_eq!(
			closing().part(Part::Known, true),
			"Recorded 2 reviews.\n  docs/a.md\n  docs/long-name.md (reread)"
		);

		let known = closing().part(Part::Known, false);
		let mut lines: Vec<String> = known.lines().map(str::to_owned).collect();
		assert_eq!(lines.len(), mark::MARK.lines().count());
		assert!(lines[REMAINING_ROW].ends_with(&output::dim(CHECKING)));
		let (row, up) = closing().remaining_row();
		assert_eq!(up, lines.len() - REMAINING_ROW);
		lines[REMAINING_ROW] = row;
		assert_eq!(
			format!(
				"{}\n{}",
				lines.join("\n"),
				closing().part(Part::Evaluated, false)
			),
			closing().text()
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
