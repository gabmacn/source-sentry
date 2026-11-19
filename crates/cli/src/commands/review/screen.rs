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
	KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::crossterm::execute;

use self::app::{App, Flow, Key, Target};
use super::closing::Writer;
use super::external;
use crate::cli::ReviewArgs;
use crate::exit::{self, Failure};
use crate::output::Printer;
use crate::workspace::Workspace;

/// Whether this terminal can show the full screen: it has colour, including
/// the deck's own three, and room for the layout.
pub(super) fn supported() -> bool {
	let room =
		ratatui::crossterm::terminal::size().is_ok_and(|(columns, rows)| {
			columns >= draw::MINIMUM.0 && rows >= draw::MINIMUM.1
		});
	crate::output::full_colour() && room
}

/// Runs a session on the full screen, then prints its closing: what the
/// session recorded at once, and what remains after a final evaluation. A
/// session with nothing to review prints its closing alone, after the same
/// final evaluation.
///
/// # Errors
///
/// Fails as the line mode does, and when the terminal cannot be taken over.
/// A write that fails after others of the same answer were recorded fails
/// once the closing has listed them.
pub(super) fn run(
	workspace: &Workspace,
	args: &ReviewArgs,
	printer: Printer,
) -> Result<(), Failure> {
	let mut app = App::new(workspace, args)?;
	review(&mut app, printer.is_quiet())
}

/// Shows the screen until the reviewer quits, unless nothing needs review,
/// then ends the session as both modes do: what it recorded at once, then
/// what remains after a final evaluation.
fn review(app: &mut App, quiet: bool) -> Result<(), Failure> {
	if !app.open.is_empty() {
		release_mouse_on_panic();
		let mut terminal = take_over()?;
		let result = events(&mut terminal, app);
		hand_back();
		result?;
	}
	// The alternate screen leaves nothing behind, so no blank line is needed.
	let mut closing = Writer::new(quiet, false);
	let closed = app
		.session
		.finish(|summary, part| closing.show(summary, part));
	app.failed.take().map_or(closed, Err)
}

/// Takes the terminal over: the alternate screen, every key as it is
/// pressed, and the mouse for its wheel, its clicks and its position.
fn take_over() -> Result<DefaultTerminal, Failure> {
	taken(
		ratatui::try_init,
		|| execute!(io::stdout(), EnableMouseCapture),
		hand_back,
	)
}

/// Takes the terminal over with `screen`, then `mouse`. When either fails,
/// what was already taken is handed back with `restore`, so that the
/// terminal is left as it was.
fn taken<T>(
	screen: impl FnOnce() -> io::Result<T>,
	mouse: impl FnOnce() -> io::Result<()>,
	restore: impl FnOnce(),
) -> Result<T, Failure> {
	let result = screen()
		.map_err(|error| failed("cannot take over the terminal", &error))
		.and_then(|terminal| {
			mouse()
				.map(|()| terminal)
				.map_err(|error| failed("cannot read the mouse", &error))
		});
	if result.is_err() {
		restore();
	}
	result
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

/// What the reviewer did: pressed a key, or clicked on something.
enum Input {
	/// A key, or a turn of the wheel.
	Key(Key),
	/// A click on something that acts on one.
	Click(Target),
	/// The pointer dragged, with its button held, over a row.
	Drag(Target),
}

/// Draws, waits for the reviewer and handles what they did, until they
/// quit. While nothing is waiting, it reads ahead the documents the reviewer
/// may open. The screen is drawn again only when something changed.
fn events(
	terminal: &mut DefaultTerminal,
	app: &mut App,
) -> Result<(), Failure> {
	let read = |error: io::Error| failed("cannot read the keyboard", &error);
	let mut stale = true;
	loop {
		if stale {
			draw(terminal, app)?;
			// What lies under the pointer may have moved away from it.
			if app.hover_changed() {
				draw(terminal, app)?;
			}
			stale = false;
		}
		while !event::poll(Duration::ZERO).map_err(read)? {
			if !app.prefetch() {
				break;
			}
		}
		let input = match event::read().map_err(read)? {
			Event::Key(pressed) => translate(pressed).map(Input::Key),
			Event::Mouse(mouse) => {
				app.pointer = Some((mouse.column, mouse.row));
				let columns = terminal
					.size()
					.map_err(|error| failed("cannot measure the terminal", &error))?
					.width;
				let input = pointed(mouse, app, columns);
				// A pointer that only moved redraws when it reached or left
				// something a click acts on.
				stale = input.is_none() && app.hover_changed();
				input
			}
			Event::Resize(..) => {
				stale = true;
				None
			}
			_ => None,
		};
		let Some(input) = input else {
			continue;
		};
		stale = true;
		let pressed = match input {
			Input::Key(key) | Input::Click(Target::Cap(key)) => Some(key),
			Input::Click(_) | Input::Drag(_) => None,
		};
		if let Some(note) = pressed.and_then(|key| app.busy(key)) {
			app.notice = Some(note.to_owned());
			draw(terminal, app)?;
		}
		let flow = match input {
			Input::Key(key) => app.key(key)?,
			Input::Click(target) => app.click(target)?,
			Input::Drag(target) => {
				app.drag(target);
				Flow::Stay
			}
		};
		match flow {
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

/// Draws the screen, and notes how many rows a page of the pane holds and
/// what a click now acts on.
fn draw(terminal: &mut DefaultTerminal, app: &mut App) -> Result<(), Failure> {
	let rows = terminal
		.size()
		.map_err(|error| failed("cannot measure the terminal", &error))?
		.height;
	app.page = usize::from(rows.saturating_sub(14)).max(1);
	let mut hits = Vec::new();
	terminal
		.draw(|frame| hits = draw::draw(app, frame.buffer_mut()))
		.map_err(|error| failed("cannot draw the review", &error))?;
	app.hits = hits;
	Ok(())
}

/// The key of a press. Windows also reports releases, which are ignored.
/// A letter typed with Control or Alt is a shortcut the screen does not
/// have, except Control and C, so it is ignored rather than taken for the
/// plain letter. Control and Alt together are kept: Windows reports `AltGr`
/// that way, and some keyboards type characters such as `@` with it. Any
/// other key the screen does not use is [`Key::Other`], which withdraws a
/// held answer; a modifier or a lock key pressed alone is no key at all.
fn translate(pressed: KeyEvent) -> Option<Key> {
	if pressed.kind == KeyEventKind::Release {
		return None;
	}
	let chord = pressed.modifiers & (KeyModifiers::CONTROL | KeyModifiers::ALT);
	Some(match pressed.code {
		KeyCode::Char('c' | 'C') if chord == KeyModifiers::CONTROL => {
			Key::Interrupt
		}
		KeyCode::Char(character)
			if chord.is_empty()
				|| chord == KeyModifiers::CONTROL | KeyModifiers::ALT =>
		{
			Key::Char(character)
		}
		KeyCode::Char(character) if character.is_alphabetic() => return None,
		KeyCode::Up if pressed.modifiers.contains(KeyModifiers::SHIFT) => {
			Key::Extend(false)
		}
		KeyCode::Down if pressed.modifiers.contains(KeyModifiers::SHIFT) => {
			Key::Extend(true)
		}
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
		KeyCode::Modifier(_)
		| KeyCode::CapsLock
		| KeyCode::NumLock
		| KeyCode::ScrollLock => return None,
		_ => Key::Other,
	})
}

/// What the mouse did, beyond moving: a turn of the wheel over a part of
/// the screen, a press of its left button on something that takes clicks,
/// or a drag over a row with that button held, which picks a range. A click
/// or a drag made with Control or Shift held is left alone: terminals give
/// those their own meaning, such as opening a path or selecting text.
fn pointed(mouse: MouseEvent, app: &App, columns: u16) -> Option<Input> {
	let area = draw::area_at(columns, mouse.column);
	let target = || app.target_at(mouse.column, mouse.row);
	let held = mouse
		.modifiers
		.intersects(KeyModifiers::CONTROL | KeyModifiers::SHIFT);
	match mouse.kind {
		MouseEventKind::ScrollDown => Some(Input::Key(Key::Wheel(true, area))),
		MouseEventKind::ScrollUp => Some(Input::Key(Key::Wheel(false, area))),
		MouseEventKind::Down(MouseButton::Left) if !held => {
			target().map(Input::Click)
		}
		MouseEventKind::Drag(MouseButton::Left) if !held => target()
			.filter(|target| matches!(target, Target::Row(_) | Target::Source(_)))
			.map(Input::Drag),
		_ => None,
	}
}

/// A terminal I/O error, with what was being done.
fn failed(context: &str, error: &io::Error) -> Failure {
	Failure::new(exit::OPERATIONAL, format!("{context}: {error}"))
}
