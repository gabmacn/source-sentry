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
use std::time::Duration;

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{
	self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent,
	KeyEventKind, KeyModifiers, MouseEvent, MouseEventKind,
};
use ratatui::crossterm::execute;

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
		release_mouse_on_panic();
		let mut terminal = take_over()?;
		let result = events(&mut terminal, &mut app);
		hand_back();
		result?;
	}
	let closing = app.closing();
	output::data(&if printer.is_quiet() {
		closing.plain()
	} else {
		closing.text()
	});
	Ok(())
}

/// Takes the terminal over: the alternate screen, every key as it is
/// pressed, and the mouse for its wheel.
fn take_over() -> Result<DefaultTerminal, Failure> {
	let terminal = ratatui::try_init()
		.map_err(|error| failed("cannot take over the terminal", &error))?;
	execute!(io::stdout(), EnableMouseCapture)
		.map_err(|error| failed("cannot read the mouse", &error))?;
	Ok(terminal)
}

/// Hands the terminal back as it was.
fn hand_back() {
	let _ = execute!(io::stdout(), DisableMouseCapture);
	ratatui::restore();
}

/// Makes a panic release the mouse too. The terminal library restores the
/// rest itself, but would leave the terminal reporting every mouse movement.
fn release_mouse_on_panic() {
	let previous = std::panic::take_hook();
	std::panic::set_hook(Box::new(move |information| {
		let _ = execute!(io::stdout(), DisableMouseCapture);
		previous(information);
	}));
}

/// Draws, waits for a key and handles it, until the reviewer quits. While
/// no key is waiting, it reads ahead the documents the reviewer may open.
fn events(
	terminal: &mut DefaultTerminal,
	app: &mut App,
) -> Result<(), Failure> {
	let read = |error: io::Error| failed("cannot read the keyboard", &error);
	loop {
		draw(terminal, app)?;
		while !event::poll(Duration::ZERO).map_err(read)? {
			if !app.prefetch() {
				break;
			}
		}
		let columns = terminal
			.size()
			.map_err(|error| failed("cannot measure the terminal", &error))?
			.width;
		let key = match event::read().map_err(read)? {
			Event::Key(pressed) => translate(pressed),
			Event::Mouse(mouse) => wheel(mouse, columns),
			// A resize, among others: draw again.
			_ => None,
		};
		let Some(key) = key else {
			continue;
		};
		if let Some(note) = app.busy(key) {
			app.notice = Some(note.to_owned());
			draw(terminal, app)?;
		}
		match app.key(key)? {
			Flow::Stay => {}
			Flow::Quit => return Ok(()),
			Flow::Edit(document) => {
				// The editor needs the terminal as it was.
				hand_back();
				let result = external::edit(app.workspace(), &document);
				*terminal = take_over()?;
				terminal
					.clear()
					.map_err(|error| failed("cannot draw the review", &error))?;
				app.edited(result)?;
			}
		}
	}
}

/// Draws the screen, and notes how many rows a page of the pane holds.
fn draw(terminal: &mut DefaultTerminal, app: &mut App) -> Result<(), Failure> {
	let rows = terminal
		.size()
		.map_err(|error| failed("cannot measure the terminal", &error))?
		.height;
	app.page = usize::from(rows.saturating_sub(14)).max(1);
	terminal
		.draw(|frame| draw::draw(app, frame.buffer_mut()))
		.map_err(|error| failed("cannot draw the review", &error))?;
	Ok(())
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

/// A turn of the wheel, with the part of the screen it happened over.
fn wheel(mouse: MouseEvent, columns: u16) -> Option<Key> {
	let down = match mouse.kind {
		MouseEventKind::ScrollDown => true,
		MouseEventKind::ScrollUp => false,
		_ => return None,
	};
	Some(Key::Wheel(down, draw::area_at(columns, mouse.column)))
}

/// A terminal I/O error, with what was being done.
fn failed(context: &str, error: &io::Error) -> Failure {
	Failure::new(exit::OPERATIONAL, format!("{context}: {error}"))
}
