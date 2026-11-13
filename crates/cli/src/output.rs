//! Where output goes, and how it is coloured.
//!
//! Text is styled with `owo-colors` and written through `anstream`, which
//! strips the styles when `--color never` is given, or under `--color auto`
//! when the stream is not a terminal or `NO_COLOR` is set.

use std::io::Write as _;

use anstream::ColorChoice;
use owo_colors::OwoColorize as _;
use source_sentry_core::model::Outcome;

use crate::cli::ColorWhen;

/// Sets the colour choice for every stream.
pub(crate) fn set_color(when: ColorWhen) {
	match when {
		ColorWhen::Auto => ColorChoice::Auto,
		ColorWhen::Always => ColorChoice::Always,
		ColorWhen::Never => ColorChoice::Never,
	}
	.write_global();
}

/// Writes a command's output, honouring `--quiet`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Printer {
	quiet: bool,
}

impl Printer {
	/// A printer that prints errors only when `quiet` is set.
	pub(crate) fn new(quiet: bool) -> Self {
		Self { quiet }
	}

	/// Whether `--quiet` was given.
	pub(crate) fn is_quiet(self) -> bool {
		self.quiet
	}

	/// Prints text for people on standard output, unless quiet.
	pub(crate) fn print(self, text: &str) {
		if !self.quiet {
			write_out(text);
		}
	}

	/// Prints a message for people on standard error, unless quiet.
	pub(crate) fn note(self, text: &str) {
		if !self.quiet {
			write_err(text);
		}
	}
}

/// Prints data that was asked for, such as JSON, on standard output, even
/// under `--quiet`: it is the command's result rather than a message.
pub(crate) fn data(text: &str) {
	write_out(text);
}

/// Prints an error on standard error, even under `--quiet`.
pub(crate) fn error(message: &str) {
	write_err(&format!("{} {message}", "error:".red().bold()));
}

/// Writes `text` and a newline to standard output. A failed write, such as
/// a closed pipe, is ignored: there is nowhere left to report it.
fn write_out(text: &str) {
	let _ = writeln!(anstream::stdout(), "{text}");
}

/// Writes `text` and a newline to standard error, ignoring a failed write.
pub(crate) fn write_err(text: &str) {
	let _ = writeln!(anstream::stderr(), "{text}");
}

/// An outcome's label, four characters wide: `fail`, `warn`, `note` or
/// `pass`, coloured by severity.
pub(crate) fn outcome_label(outcome: Outcome) -> String {
	let name = outcome_name(outcome);
	match outcome {
		Outcome::Fail => name.red().bold().to_string(),
		Outcome::Warn => name.yellow().bold().to_string(),
		Outcome::Notice => name.cyan().bold().to_string(),
		Outcome::Pass => name.green().to_string(),
	}
}

/// An outcome's label without colour: `fail`, `warn`, `note` or `pass`.
pub(crate) fn outcome_name(outcome: Outcome) -> &'static str {
	match outcome {
		Outcome::Fail => "fail",
		Outcome::Warn => "warn",
		Outcome::Notice => "note",
		Outcome::Pass => "pass",
	}
}

/// Text for a good state, in the colour of a passing outcome.
pub(crate) fn good(text: &str) -> String {
	text.green().to_string()
}

/// Bold text.
pub(crate) fn bold(text: &str) -> String {
	text.bold().to_string()
}

/// Dimmed text, for secondary detail such as origins.
pub(crate) fn dim(text: &str) -> String {
	text.dimmed().to_string()
}

/// Text for an addition, as in a diff.
pub(crate) fn added(text: &str) -> String {
	text.green().to_string()
}

/// Text for a removal, as in a diff.
pub(crate) fn removed(text: &str) -> String {
	text.red().to_string()
}

/// Text for a diff's hunk header.
pub(crate) fn hunk(text: &str) -> String {
	text.cyan().to_string()
}

/// A diagnostic severity's name, coloured: errors red, warnings yellow.
pub(crate) fn severity(name: &str, fatal: bool) -> String {
	if fatal {
		name.red().bold().to_string()
	} else {
		name.yellow().bold().to_string()
	}
}

/// `plain` followed by spaces up to `width` characters, as `styled`.
pub(crate) fn pad(styled: &str, plain: &str, width: usize) -> String {
	let padding = width.saturating_sub(plain.chars().count());
	format!("{styled}{}", " ".repeat(padding))
}
