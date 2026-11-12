//! Inquire prompts on the user's terminal. Text input preserves the one-letter
//! answers and the empty answer, while Confirm protects an unseen batch.

use std::io;

use inquire::ui::RenderConfig;
use inquire::validator::Validation;
use inquire::{Confirm, InquireError, Text};
use source_sentry_core::model::DocumentId;
use source_sentry_core::mutate::DocumentDiff;

use super::{Answer, Interface, View};
use crate::exit::{self, Failure};
use crate::output;
use crate::workspace::Workspace;

/// The production interaction boundary, using inquire's crossterm backend.
pub(super) struct Terminal;

impl Interface for Terminal {
	fn show(&mut self, workspace: &Workspace, view: &View) {
		output::data(&super::render::document(workspace, view));
	}

	fn answer(&mut self) -> Result<Option<Answer>, Failure> {
		let answer = Text::new("Review [n/y/r/a/d/e/q]")
			.with_default("n")
			.with_render_config(render_config())
			.with_help_message("n/Enter leave, y attest, r reread, a all, d full diff, e edit, q quit; submit with Enter")
			.with_validator(|value: &str| Ok(if Answer::parse(value).is_some() {
				Validation::Valid
			} else {
				Validation::Invalid("Enter n, y, r, a, d, e or q.".into())
			}))
			.prompt();
		Ok(answer_or_end(answer)?.and_then(|value| Answer::parse(&value)))
	}

	fn confirm_all(
		&mut self,
		remaining: usize,
		unseen: usize,
	) -> Result<Option<bool>, Failure> {
		answer_or_end(
			Confirm::new(&format!(
				"Record the remaining {}, including {unseen} you have not seen?",
				crate::describe::count(remaining, "document", "documents")
			))
			.with_default(false)
			.with_render_config(render_config())
			.prompt(),
		)
	}

	fn full_diff(
		&mut self,
		workspace: &Workspace,
		compared: &DocumentDiff,
	) -> Result<(), Failure> {
		super::external::full_diff(workspace, compared)
	}

	fn edit(
		&mut self,
		workspace: &Workspace,
		document: &DocumentId,
	) -> Result<(), Failure> {
		super::external::edit(workspace, document)
	}

	fn message(&mut self, text: &str) {
		output::data(text);
	}
}

/// Applies the CLI's explicit colour choice to inquire as well as to normal
/// output. Auto uses inquire's default, which respects `NO_COLOR`.
fn render_config() -> RenderConfig<'static> {
	match anstream::ColorChoice::global() {
		anstream::ColorChoice::Never => RenderConfig::empty(),
		anstream::ColorChoice::Always => RenderConfig::default_colored(),
		_ => RenderConfig::default(),
	}
}

/// Cancellation, interruption and end of input mean quit. Other terminal I/O
/// errors retain an operational exit code instead of accepting an answer.
fn answer_or_end<T>(
	answer: Result<T, InquireError>,
) -> Result<Option<T>, Failure> {
	match answer {
		Ok(answer) => Ok(Some(answer)),
		Err(
			InquireError::OperationCanceled | InquireError::OperationInterrupted,
		) => Ok(None),
		Err(InquireError::IO(error))
			if matches!(
				error.kind(),
				io::ErrorKind::UnexpectedEof | io::ErrorKind::NotConnected
			) =>
		{
			Ok(None)
		}
		Err(error) => Err(Failure::new(
			exit::OPERATIONAL,
			format!("cannot read the review answer: {error}"),
		)),
	}
}

#[cfg(test)]
mod tests {
	use std::io;

	use inquire::InquireError;

	use super::answer_or_end;

	/// Cancellation and EOF have the same quit semantics for every prompt type.
	#[test]
	fn cancellation_interruption_and_eof_end_the_session() {
		for error in [
			InquireError::OperationCanceled,
			InquireError::OperationInterrupted,
			InquireError::IO(io::Error::from(io::ErrorKind::UnexpectedEof)),
		] {
			assert_eq!(answer_or_end::<bool>(Err(error)).unwrap(), None);
		}
		assert_eq!(answer_or_end(Ok(false)).unwrap(), Some(false));
	}

	/// Other prompt failures are operational errors, never affirmative answers.
	#[test]
	fn terminal_io_errors_refuse_the_answer() {
		assert_eq!(
			answer_or_end::<bool>(Err(InquireError::IO(io::Error::from(
				io::ErrorKind::PermissionDenied
			))))
			.unwrap_err()
			.code,
			4
		);
	}
}
