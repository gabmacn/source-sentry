//! The review on the full screen: the documents in a sidebar, the one on
//! display in a pane, and the question in one fixed place. The reviewer
//! chooses the order; the session underneath is the line mode's.
//!
//! The screen is drawn on the terminal's alternate screen, which leaves no
//! trace, so the closing is printed once the terminal has been handed back.

mod app;
mod draw;

#[cfg(test)]
mod tests;

use std::io;

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{
	self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
};

use self::app::{App, Flow, Key};
use super::external;
use crate::cli::ReviewArgs;
use crate::exit::{self, Failure};
use crate::output::{self, Printer};
use crate::workspace::Workspace;

/// Whether this terminal can show the full screen: it has colour, including
/// the deck's own three, and room for the layout.
pub(super) fn supported() -> bool {
	let colour = !matches!(
		anstream::AutoStream::choice(&io::stdout()),
		anstream::ColorChoice::Never
	);
	// Windows consoles have taken 24-bit colour since Windows 10; elsewhere
	// a terminal announces it.
	let deck = cfg!(windows)
		|| std::env::var("COLORTERM")
			.is_ok_and(|value| value == "truecolor" || value == "24bit");
	let room =
		ratatui::crossterm::terminal::size().is_ok_and(|(columns, rows)| {
			columns >= draw::MINIMUM.0 && rows >= draw::MINIMUM.1
		});
	colour && deck && room
}

/// Runs a session on the full screen, then prints its closing. A session
/// with nothing to review prints the closing alone.
///
/// # Errors
///
/// Fails as the line mode does, and when the terminal cannot be taken over.
pub(super) fn run(
	workspace: &Workspace,
	args: &ReviewArgs,
	printer: Printer,
) -> Result<(), Failure> {
	let mut app = App::new(workspace, args)?;
	if !app.open.is_empty() {
		let mut terminal = ratatui::try_init()
			.map_err(|error| failed("cannot take over the terminal", &error))?;
		let result = events(&mut terminal, &mut app);
		ratatui::restore();
		result?;
	}
	let closing = app.closing()?;
	output::data(&if printer.is_quiet() {
		closing.plain()
	} else {
		closing.text()
	});
	Ok(())
}

/// Draws, waits for a key and handles it, until the reviewer quits.
fn events(
	terminal: &mut DefaultTerminal,
	app: &mut App,
) -> Result<(), Failure> {
	loop {
		let rows = terminal
			.size()
			.map_err(|error| failed("cannot measure the terminal", &error))?
			.height;
		app.page = usize::from(rows.saturating_sub(14)).max(1);
		terminal
			.draw(|frame| draw::draw(app, frame.buffer_mut()))
			.map_err(|error| failed("cannot draw the review", &error))?;
		let event = event::read()
			.map_err(|error| failed("cannot read the keyboard", &error))?;
		let Event::Key(pressed) = event else {
			// A resize, among others: draw again.
			continue;
		};
		let Some(key) = translate(pressed) else {
			continue;
		};
		match app.key(key)? {
			Flow::Stay => {}
			Flow::Quit => return Ok(()),
			Flow::Edit(document) => {
				// The editor needs the terminal as it was.
				ratatui::restore();
				let result = external::edit(app.workspace(), &document);
				*terminal = ratatui::try_init()
					.map_err(|error| failed("cannot take the terminal back", &error))?;
				terminal
					.clear()
					.map_err(|error| failed("cannot draw the review", &error))?;
				app.edited(result)?;
			}
		}
	}
}

/// The key of a press. Windows also reports releases, which are ignored.
fn translate(pressed: KeyEvent) -> Option<Key> {
	if pressed.kind == KeyEventKind::Release {
		return None;
	}
	Some(match pressed.code {
		KeyCode::Char('c' | 'C')
			if pressed.modifiers.contains(KeyModifiers::CONTROL) =>
		{
			Key::Interrupt
		}
		KeyCode::Char(character) => Key::Char(character),
		KeyCode::Up => Key::Up,
		KeyCode::Down => Key::Down,
		KeyCode::Left => Key::Left,
		KeyCode::Right => Key::Right,
		KeyCode::Enter => Key::Enter,
		KeyCode::Esc => Key::Escape,
		KeyCode::BackTab => Key::BackTab,
		KeyCode::Tab if pressed.modifiers.contains(KeyModifiers::SHIFT) => {
			Key::BackTab
		}
		KeyCode::Tab => Key::Tab,
		KeyCode::PageUp => Key::PageUp,
		KeyCode::PageDown => Key::PageDown,
		KeyCode::Backspace => Key::Backspace,
		_ => return None,
	})
}

/// A terminal I/O error, with what was being done.
fn failed(context: &str, error: &io::Error) -> Failure {
	Failure::new(exit::OPERATIONAL, format!("{context}: {error}"))
}
