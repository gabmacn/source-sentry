//! Draws the full screen into a grid of cells: a header of session facts, a
//! sidebar of documents, a pane for the document on display and, in one
//! fixed place, the question.
//!
//! The colours are the deck's: an ink ground, cream text, and blue only as
//! a fill, never as text.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use source_sentry_core::model::Outcome;

use super::super::changes::Row;
use super::app::{App, Area, Cursor, Field, Kind, Overlay, Pane, Shown};
use crate::{mark, output};

/// The smallest screen the layout fits: columns, then rows.
pub(super) const MINIMUM: (u16, u16) = (80, 20);
/// The sidebar's width.
const SIDEBAR: u16 = 30;
/// The width from which the changes and the document sit side by side.
const WIDE: u16 = 200;
/// The rows the header and the question take around the body.
const HEADER: u16 = 2;
/// The rows of the question: a rule, the question and the keys' line.
const QUESTION: u16 = 3;

const INK: Color = Color::Rgb(22, 19, 14);
const CREAM: Color = Color::Rgb(241, 230, 208);
const BLUE: Color = Color::Rgb(31, 43, 255);
const DIM: Color = Color::Rgb(149, 141, 127);
const FAINT: Color = Color::Rgb(75, 70, 61);
const DIM_BLUE: Color = Color::Rgb(25, 26, 86);
const ADDED: (Color, Color) =
	(Color::Rgb(195, 230, 171), Color::Rgb(26, 41, 21));
const REMOVED: (Color, Color) =
	(Color::Rgb(243, 174, 164), Color::Rgb(47, 26, 24));

/// Cream text on the ground.
fn text() -> Style {
	Style::new().fg(CREAM)
}

/// Bold cream text.
fn strong() -> Style {
	text().add_modifier(Modifier::BOLD)
}

/// Secondary text.
fn dim() -> Style {
	Style::new().fg(DIM)
}

/// Rules and borders.
fn faint() -> Style {
	Style::new().fg(FAINT)
}

/// The selection: cream on blue.
fn selected() -> Style {
	Style::new().fg(CREAM).bg(BLUE)
}

/// A key's cap: ink on cream.
fn cap() -> Style {
	Style::new().fg(INK).bg(CREAM).add_modifier(Modifier::BOLD)
}

/// An outcome's colour.
fn outcome_color(outcome: Outcome) -> Color {
	match outcome {
		Outcome::Fail => Color::Rgb(240, 86, 74),
		Outcome::Warn => Color::Rgb(232, 185, 60),
		Outcome::Notice => Color::Rgb(92, 200, 214),
		Outcome::Pass => Color::Rgb(143, 199, 107),
	}
}

/// The columns `text` takes.
fn width(text: &str) -> u16 {
	u16::try_from(Span::raw(text).width()).unwrap_or(u16::MAX)
}

/// `text` cut to `columns`, with an ellipsis when something was cut.
fn fit(text: &str, columns: u16) -> String {
	if width(text) <= columns {
		return text.to_owned();
	}
	let mut cut: String = text
		.chars()
		.take(usize::from(columns.saturating_sub(1)))
		.collect();
	cut.push('…');
	cut
}

/// A path cut to `columns` in its middle, so that the file's name stays.
fn fit_path(path: &str, columns: u16) -> String {
	if width(path) <= columns {
		return path.to_owned();
	}
	let Some((directory, file)) = path.rsplit_once('/') else {
		return fit(path, columns);
	};
	let room = usize::from(columns).saturating_sub(file.chars().count() + 2);
	if room < 3 {
		return fit(path, columns);
	}
	let head: String = directory.chars().take(room).collect();
	format!("{head}…/{file}")
}

/// `text` in lines of at most `columns` characters.
fn wrap(text: &str, columns: u16) -> Vec<String> {
	let characters: Vec<char> = text.chars().collect();
	if characters.is_empty() {
		return vec![String::new()];
	}
	characters
		.chunks(usize::from(columns.max(1)))
		.map(|chunk| chunk.iter().collect())
		.collect()
}

/// A pen over the grid. Everything it draws is clipped to the grid.
struct Pen<'b> {
	/// The grid.
	buffer: &'b mut Buffer,
}

impl Pen<'_> {
	/// Writes `text` from `(x, y)` in at most `columns` cells, and returns
	/// the column after it.
	fn text(
		&mut self,
		x: u16,
		y: u16,
		text: &str,
		style: Style,
		columns: u16,
	) -> u16 {
		if y >= self.buffer.area.bottom() || x >= self.buffer.area.right() {
			return x;
		}
		let room = columns.min(self.buffer.area.right() - x);
		self
			.buffer
			.set_stringn(x, y, text, usize::from(room), style)
			.0
	}

	/// Writes `text` so that it ends before column `end`, and returns the
	/// column where it starts.
	fn right(&mut self, end: u16, y: u16, text: &str, style: Style) -> u16 {
		let start = end.saturating_sub(width(text));
		self.text(start, y, text, style, end - start);
		start
	}

	/// Gives every cell of `area` this style.
	fn fill(&mut self, area: Rect, style: Style) {
		self
			.buffer
			.set_style(area.intersection(self.buffer.area), style);
	}

	/// Repeats `symbol` over `columns` cells of row `y`.
	fn rule(&mut self, x: u16, y: u16, columns: u16, symbol: &str, style: Style) {
		for column in x..x.saturating_add(columns) {
			self.text(column, y, symbol, style, 1);
		}
	}

	/// Writes key caps with their labels from `(x, y)`.
	fn keys(&mut self, mut x: u16, y: u16, keys: &[(&str, &str)]) {
		for (key, label) in keys {
			x = self.text(x, y, &format!(" {key} "), cap(), u16::MAX);
			x = self.text(x + 1, y, label, text(), u16::MAX) + 4;
		}
	}
}

/// The part of a screen `columns` wide that column `x` belongs to.
pub(super) fn area_at(columns: u16, x: u16) -> Area {
	if x <= SIDEBAR {
		Area::Sidebar
	} else if columns < WIDE {
		Area::Pane
	} else if x <= SIDEBAR + 3 + (columns - SIDEBAR - 6) / 2 {
		Area::Changes
	} else {
		Area::Document
	}
}

/// Draws the whole screen.
pub(super) fn draw(app: &App, buffer: &mut Buffer) {
	let area = buffer.area;
	let mut pen = Pen { buffer };
	pen.fill(area, Style::new().fg(CREAM).bg(INK));
	if area.width < MINIMUM.0 || area.height < MINIMUM.1 {
		let note = format!(
			"Source Sentry needs {} by {} cells; this window has {} by {}.",
			MINIMUM.0, MINIMUM.1, area.width, area.height
		);
		pen.text(
			1,
			area.height / 2,
			&note,
			text(),
			area.width.saturating_sub(2),
		);
		return;
	}
	let body = Rect::new(0, HEADER, area.width, area.height - HEADER - QUESTION);
	header(app, &mut pen, area.width);
	pen.rule(0, HEADER - 1, area.width, "─", faint());
	pen.rule(0, body.bottom(), area.width, "─", faint());
	for y in body.y..body.bottom() {
		pen.text(SIDEBAR, y, "│", faint(), 1);
	}
	pen.text(SIDEBAR, HEADER - 1, "┬", faint(), 1);
	pen.text(SIDEBAR, body.bottom(), "┴", faint(), 1);
	sidebar(app, &mut pen, Rect::new(0, body.y, SIDEBAR, body.height));
	let right =
		Rect::new(SIDEBAR + 2, body.y, area.width - SIDEBAR - 3, body.height);
	match &app.shown {
		Some(shown) => pane(app, shown, &mut pen, right),
		None => {
			pen.text(
				right.x,
				right.y,
				"Nothing is left to review.",
				dim(),
				right.width,
			);
		}
	}
	question(app, &mut pen, body.bottom() + 1, area.width);
	if let Some(overlay) = &app.overlay {
		step_back(pen.buffer);
		overlay_box(app, overlay, &mut pen, area);
	}
}

/// The product, the session's facts and its progress.
fn header(app: &App, pen: &mut Pen, columns: u16) {
	let mut x = pen.text(
		0,
		0,
		" SOURCE SENTRY ",
		selected().add_modifier(Modifier::BOLD),
		u16::MAX,
	) + 3;
	let projects = app.session.state.projects.len().to_string();
	for (field, label, value) in [
		(
			Field::Reviewer,
			"Reviewer",
			app.session.reviewer.as_deref().unwrap_or("not set"),
		),
		(Field::Context, "Context", app.session.context.as_str()),
		(Field::Projects, "Projects", projects.as_str()),
	] {
		let on = app.cursor == Cursor::Header(field);
		let start = x;
		x = pen.text(
			x,
			0,
			&format!(" {label} "),
			if on { selected() } else { dim() },
			u16::MAX,
		);
		x = pen.text(
			x,
			0,
			&format!("{} ", fit(value, 24)),
			if on { selected() } else { text() },
			u16::MAX,
		);
		if on {
			pen.fill(Rect::new(start, 0, x - start, 1), selected());
		}
		x += 1;
	}
	let (done, total) = app.progress();
	let bar: u16 = 14;
	let filled = (usize::from(bar) * done)
		.checked_div(total)
		.and_then(|filled| u16::try_from(filled).ok())
		.unwrap_or(bar);
	let end = columns - 1;
	pen.rule(end - bar, 0, filled, "█", Style::new().fg(BLUE));
	pen.rule(
		end - bar + filled,
		0,
		bar - filled,
		"░",
		Style::new().fg(BLUE),
	);
	pen.right(
		end - bar - 2,
		0,
		&format!("{done} of {total} reviewed"),
		dim(),
	);
}

/// The documents that need review, then all the others, then the mark when
/// there is room for it.
fn sidebar(app: &App, pen: &mut Pen, area: Rect) {
	let y = queue(app, pen, area) + 1;
	if y >= area.bottom() {
		return;
	}
	let arrow = if app.expanded { "▾" } else { "▸" };
	pen.text(
		area.x + 1,
		y,
		&format!("{arrow} OTHER DOCUMENTS"),
		dim().add_modifier(Modifier::BOLD),
		u16::MAX,
	);
	pen.right(area.right() - 1, y, &app.others.len().to_string(), dim());
	let below = Rect::new(
		area.x,
		y + 1,
		area.width,
		area.bottom().saturating_sub(y + 1),
	);
	if app.expanded {
		others(app, pen, below);
	} else {
		glow(pen, below);
	}
}

/// The documents that need review, two rows each, around the cursor when
/// they do not all fit. Returns the row after the last.
fn queue(app: &App, pen: &mut Pen, area: Rect) -> u16 {
	let workspace = app.workspace();
	let name_width = area.width - 8;
	let mut y = area.y;
	pen.text(
		area.x + 1,
		y,
		"NEEDS REVIEW",
		dim().add_modifier(Modifier::BOLD),
		u16::MAX,
	);
	pen.right(area.right() - 1, y, &app.open.len().to_string(), dim());
	y += 1;
	// The queue keeps the rows it needs, and leaves three for what follows.
	let room = usize::from(area.height.saturating_sub(4) / 2).max(1);
	let at = match app.cursor {
		Cursor::Open(index) => index,
		_ => 0,
	};
	for (index, id) in app
		.open
		.iter()
		.enumerate()
		.skip(at.saturating_sub(room - 1))
		.take(room)
	{
		let on = app.cursor == Cursor::Open(index);
		if on {
			pen.fill(Rect::new(area.x, y, area.width, 2), selected());
		}
		if let Some(document) = app.session.state.documents.get(id) {
			let outcome = document.outcome();
			let label = if on {
				selected().add_modifier(Modifier::BOLD)
			} else {
				Style::new()
					.fg(outcome_color(outcome))
					.add_modifier(Modifier::BOLD)
			};
			pen.text(area.x + 1, y, output::outcome_name(outcome), label, 4);
			pen.text(
				area.x + 7,
				y + 1,
				&fit(&document.facts.replace(", ", " · "), name_width),
				if on { selected() } else { dim() },
				name_width,
			);
		}
		pen.text(
			area.x + 7,
			y,
			&fit_path(&id.display(workspace), name_width),
			if on {
				selected().add_modifier(Modifier::BOLD)
			} else {
				text()
			},
			name_width,
		);
		y += 2;
	}
	y
}

/// Every other document, one row each, around the cursor when they do not
/// all fit.
fn others(app: &App, pen: &mut Pen, area: Rect) {
	let workspace = app.workspace();
	let name_width = area.width - 8;
	let room = usize::from(area.height);
	let at = match app.cursor {
		Cursor::Other(index) => index,
		_ => 0,
	};
	let first = at.saturating_sub(room.saturating_sub(1));
	for (y, (index, other)) in
		(area.y..).zip(app.others.iter().enumerate().skip(first).take(room))
	{
		let on = app.cursor == Cursor::Other(index);
		if on {
			pen.fill(Rect::new(area.x, y, area.width, 1), selected());
		}
		let (label, color, name) = match other.kind {
			Kind::Reviewed => (" ✓", outcome_color(Outcome::Pass), text()),
			Kind::Live(outcome) => {
				(output::outcome_name(outcome), outcome_color(outcome), dim())
			}
			Kind::Record => ("seal", DIM, dim()),
		};
		pen.text(
			area.x + 1,
			y,
			label,
			if on {
				selected()
			} else {
				Style::new().fg(color)
			},
			4,
		);
		pen.text(
			area.x + 7,
			y,
			&fit_path(&other.id.display(workspace), name_width),
			if on { selected() } else { name },
			name_width,
		);
	}
}

/// The mark in the sidebar's empty lower part, on a field of blue shade
/// characters that thins away from the eye: ordered dithering, in cells.
fn glow(pen: &mut Pen, area: Rect) {
	let lines: Vec<&str> = mark::MARK.lines().collect();
	let height = u16::try_from(lines.len()).unwrap_or(u16::MAX);
	if area.height < height + 2 || area.width < 24 {
		return;
	}
	let top = area.bottom() - height - 1;
	let left = area.x + (area.width - 24) / 2;
	for y in top - 1..=top + height {
		for x in area.x..area.right() {
			let symbol = y
				.checked_sub(top)
				.and_then(|row| lines.get(usize::from(row)))
				.and_then(|line| line.chars().nth(usize::from(x.saturating_sub(left))))
				.filter(|symbol| *symbol != ' ' && x >= left);
			if let Some(symbol) = symbol {
				pen.text(x, y, &symbol.to_string(), Style::new().fg(CREAM).bg(INK), 1);
				continue;
			}
			// A cell is about twice as tall as it is wide.
			let across = f32::from(x) + 0.5 - f32::from(left + 12);
			let down = (f32::from(y) + 0.5 - f32::from(top + height / 2)) * 2.0;
			let distance = across.hypot(down);
			if distance < 8.6 {
				pen.text(x, y, "▒", Style::new().fg(BLUE), 1);
			} else if distance < 12.6 {
				pen.text(x, y, "░", Style::new().fg(BLUE), 1);
			}
		}
	}
}

/// The document on display: its name and details, then its changes, its
/// text, or on a wide screen both.
fn pane(app: &App, shown: &Shown, pen: &mut Pen, area: Rect) {
	pen.text(
		area.x,
		area.y,
		&fit_path(&shown.name, area.width.saturating_sub(30)),
		strong(),
		area.width,
	);
	let end = pen.right(
		area.right(),
		area.y,
		&shown.facts.replace(", ", " · "),
		text(),
	);
	if let Some(outcome) = shown.outcome {
		pen.right(
			end - 2,
			area.y,
			output::outcome_name(outcome),
			Style::new()
				.fg(outcome_color(outcome))
				.add_modifier(Modifier::BOLD),
		);
	}
	let mut x = area.x;
	for (label, value) in [
		(
			"Reviewed",
			shown.reviewed.clone().unwrap_or_else(|| "never".to_owned()),
		),
		("Project", app.workspace().display_path(&shown.id.project)),
		(
			"Waits on",
			if shown.upstreams.is_empty() {
				"nothing".to_owned()
			} else {
				shown.upstreams.join(", ")
			},
		),
	] {
		x = pen.text(x, area.y + 1, &format!("{label} "), dim(), u16::MAX);
		x = pen.text(
			x,
			area.y + 1,
			&value,
			text(),
			area.right().saturating_sub(x),
		) + 3;
	}
	let mut top = area.y + 3;
	if let Some(reason) = &shown.reason {
		pen.text(area.x, area.y + 2, reason, text(), area.width);
		top += 1;
	}
	let below =
		Rect::new(area.x, top, area.width, area.bottom().saturating_sub(top));
	if area.width + SIDEBAR + 3 >= WIDE {
		let half = (below.width - 3) / 2;
		let divider = below.x + half + 1;
		for y in below.y..below.bottom() {
			pen.text(divider, y, "│", faint(), 1);
		}
		changes(
			app,
			shown,
			pen,
			Rect::new(below.x, below.y, half, below.height),
		);
		document(
			app,
			shown,
			pen,
			Rect::new(
				divider + 2,
				below.y,
				below.right() - divider - 2,
				below.height,
			),
		);
	} else if app.pane == Pane::Changes {
		changes(app, shown, pen, below);
	} else {
		document(app, shown, pen, below);
	}
}

/// The changed files, then the selected file's changes.
fn changes(app: &App, shown: &Shown, pen: &mut Pen, area: Rect) {
	let heading = if app.pane == Pane::Changes {
		strong()
	} else {
		dim().add_modifier(Modifier::BOLD)
	};
	pen.text(area.x, area.y, "SOURCES", heading, u16::MAX);
	pen.right(
		area.right(),
		area.y,
		&format!("{} changed of {}", shown.files.len(), shown.total),
		dim(),
	);
	if shown.files.is_empty() {
		let note = if shown.outcome.is_none() {
			"A sealed record must not change, so there is nothing to compare."
		} else if shown.compared.is_none() {
			"This document could not be compared."
		} else if shown.total == 0 {
			"No sources: recording a review records a whole-document reread."
		} else if shown.pending {
			"No source content changed; the facts above still require review."
		} else {
			"Nothing changed since the last review."
		};
		pen.text(
			area.x + 1,
			area.y + 1,
			note,
			dim(),
			area.width.saturating_sub(1),
		);
		return;
	}
	let y = files(app, shown, pen, area);
	pen.rule(area.x, y, area.width, "─", faint());
	let below = Rect::new(
		area.x,
		y + 1,
		area.width,
		area.bottom().saturating_sub(y + 1),
	);
	rows(app, pen, below);
	scrollbar(pen, area, app.scroll.0, app.rows().len());
}

/// The changed files with their counts and object names: at most five at
/// once, around the selected one. Returns the row after the last.
fn files(app: &App, shown: &Shown, pen: &mut Pen, area: Rect) -> u16 {
	let mut y = area.y + 1;
	for (index, file) in shown
		.files
		.iter()
		.enumerate()
		.skip(app.file.saturating_sub(4))
		.take(5)
	{
		let on = index == app.file;
		if on {
			pen.fill(Rect::new(area.x, y, area.width, 1), selected());
		}
		let plain = |style: Style| if on { selected() } else { style };
		let mut start = pen.right(
			area.right() - 1,
			y,
			&format!("{} → {}", file.old, file.new),
			plain(dim()),
		);
		if let Some((added, removed)) = file.counts {
			start = pen.right(
				start - 3,
				y,
				&format!("-{removed}"),
				plain(Style::new().fg(REMOVED.0)),
			);
			start = pen.right(
				start - 1,
				y,
				&format!("+{added}"),
				plain(Style::new().fg(ADDED.0)),
			);
		}
		let room = start.saturating_sub(area.x + 4);
		pen.text(
			area.x + 1,
			y,
			&fit_path(&file.path, room),
			if on {
				selected().add_modifier(Modifier::BOLD)
			} else {
				text()
			},
			room,
		);
		y += 1;
	}
	y
}

/// The selected file's rows from where they are scrolled to: a line where
/// each group of changes starts, then numbered lines on a tinted ground
/// when added or removed.
fn rows(app: &App, pen: &mut Pen, area: Rect) {
	let code = area.width.saturating_sub(9);
	let mut y = area.y;
	for row in app.rows().iter().skip(app.scroll.0) {
		let (number, sign, value) = match row {
			Row::Hunk(line) => (String::new(), ' ', format!("from line {line}")),
			Row::NoNewline => (
				String::new(),
				' ',
				"\\ No newline at end of file".to_owned(),
			),
			Row::Note(note) => (String::new(), ' ', (*note).to_owned()),
			Row::Line { number, sign, text } => {
				(number.to_string(), *sign, text.clone())
			}
		};
		let (ink, ground) = match sign {
			'+' => (ADDED.0, Some(ADDED.1)),
			'-' => (REMOVED.0, Some(REMOVED.1)),
			_ if number.is_empty() => (DIM, None),
			_ => (CREAM, None),
		};
		for (part, line) in wrap(&value, code).iter().enumerate() {
			if y >= area.bottom() {
				return;
			}
			if let Some(ground) = ground {
				pen.fill(
					Rect::new(area.x, y, area.width.saturating_sub(1), 1),
					Style::new().bg(ground),
				);
			}
			if part == 0 {
				pen.right(
					area.x + 4,
					y,
					&number,
					Style::new().fg(if sign == ' ' { DIM } else { ink }),
				);
				pen.text(
					area.x + 5,
					y,
					&sign.to_string(),
					Style::new().fg(ink).add_modifier(Modifier::BOLD),
					1,
				);
			}
			pen.text(area.x + 7, y, line, Style::new().fg(ink), code);
			y += 1;
		}
	}
}

/// The document's own text, numbered.
fn document(app: &App, shown: &Shown, pen: &mut Pen, area: Rect) {
	let active = app.pane == Pane::Document;
	pen.text(
		area.x,
		area.y,
		"THE DOCUMENT",
		if active {
			strong()
		} else {
			dim().add_modifier(Modifier::BOLD)
		},
		u16::MAX,
	);
	let lines = match &shown.text {
		Ok(lines) => lines,
		Err(reason) => {
			pen.text(
				area.x + 1,
				area.y + 1,
				reason,
				dim(),
				area.width.saturating_sub(1),
			);
			return;
		}
	};
	pen.right(
		area.right(),
		area.y,
		&format!("{} lines", lines.len()),
		dim(),
	);
	pen.rule(area.x, area.y + 1, area.width, "─", faint());
	let mut y = area.y + 2;
	let columns = area.width.saturating_sub(7);
	let mut fenced = false;
	'lines: for (index, line) in lines.iter().enumerate() {
		let fence = line.trim_start().starts_with("```");
		let style = if line.starts_with('#') {
			strong()
		} else if fence || fenced {
			dim()
		} else {
			text()
		};
		if fence {
			fenced = !fenced;
		}
		if index < app.scroll.1 {
			continue;
		}
		for (part, piece) in wrap(line, columns).iter().enumerate() {
			if y >= area.bottom() {
				break 'lines;
			}
			if part == 0 {
				pen.right(area.x + 4, y, &(index + 1).to_string(), dim());
			}
			pen.text(area.x + 6, y, piece, style, columns);
			y += 1;
		}
	}
	scrollbar(pen, area, app.scroll.1, lines.len());
}

/// A thumb at the pane's right edge when its rows do not all fit.
fn scrollbar(pen: &mut Pen, area: Rect, at: usize, rows: usize) {
	let height = usize::from(area.height.saturating_sub(2));
	if rows <= height || height == 0 {
		return;
	}
	let thumb = (height * height / rows).max(1);
	let offset = (at * (height - thumb)) / (rows - 1).max(1);
	for row in 0..height {
		let style = if (offset..offset + thumb).contains(&row) {
			dim()
		} else {
			faint()
		};
		let y = area.y + 2 + u16::try_from(row).unwrap_or(u16::MAX);
		pen.text(area.right() - 1, y, "▐", style, 1);
	}
}

/// One form of a key's label: its cap, its full label and its short one.
type Answer<'t> = (&'t str, &'t str, &'t str);

/// The question about the document on display, with its keys, and below it
/// either a notice or the keys that move around. A narrow screen gets the
/// short form of the question, then of the labels.
fn question(app: &App, pen: &mut Pen, y: u16, columns: u16) {
	let (long, short, keys): (String, &str, &[Answer]) =
		match (&app.shown, app.armed, app.cursor) {
			(_, _, Cursor::Header(_)) => (
				"The session's settings.".to_owned(),
				"Settings.",
				&[
					("Enter", "change", "change"),
					("← →", "move", "move"),
					("↓", "back to the documents", "back"),
				],
			),
			(Some(shown), Some(reread), _) => (
				format!(
					"Record that {} is still accurate{}?",
					shown.name,
					if reread { ", reread in full" } else { "" }
				),
				if reread {
					"Record it, reread in full?"
				} else {
					"Record it?"
				},
				&[("Enter", "record", "record"), ("Esc", "cancel", "cancel")],
			),
			(Some(shown), None, _) if shown.pending => (
				format!("Is {} still accurate?", shown.name),
				"Still accurate?",
				&[
					("y", "yes", "yes"),
					("r", "yes, and I reread all of it", "yes, reread"),
					("e", "edit it", "edit"),
					("a", "yes to all", "yes to all"),
				],
			),
			(Some(shown), None, _) if shown.outcome.is_some() => (
				"Nothing changed since the last review.".to_owned(),
				"Nothing changed.",
				&[("r", "record a reread", "reread"), ("e", "edit it", "edit")],
			),
			(Some(_), None, _) => (
				"A sealed record: there is nothing to answer.".to_owned(),
				"A sealed record.",
				&[],
			),
			(None, _, _) => (
				"Nothing is left to review.".to_owned(),
				"Nothing is left.",
				&[("q", "close", "close")],
			),
		};
	let needs = |sentence: &str, brief: bool| {
		keys
			.iter()
			.fold(1 + width(sentence), |total, (key, label, short)| {
				total + 7 + width(key) + width(if brief { short } else { label })
			})
	};
	let (sentence, brief) = if needs(&long, false) < columns {
		(long.as_str(), false)
	} else if needs(short, false) < columns {
		(short, false)
	} else {
		(short, true)
	};
	let labels: Vec<(&str, &str)> = keys
		.iter()
		.map(|(key, label, short)| (*key, if brief { *short } else { *label }))
		.collect();
	let x = pen.text(1, y, sentence, strong(), columns.saturating_sub(2)) + 4;
	pen.keys(x, y, &labels);
	if let Some(notice) = &app.notice {
		pen.text(1, y + 1, notice, text(), columns.saturating_sub(2));
		return;
	}
	let swap = match app.pane {
		Pane::Changes => "document",
		Pane::Document => "changes",
	};
	pen.text(
		1,
		y + 1,
		&format!("? keys   q quit   ↑↓ document   Tab source   v {swap}   o other documents   PgUp PgDn scroll"),
		dim(),
		columns.saturating_sub(2),
	);
}

/// Sends the screen behind a box back: its text fades and its fills dim.
fn step_back(buffer: &mut Buffer) {
	for cell in &mut buffer.content {
		cell.modifier = Modifier::empty();
		if cell.bg == BLUE {
			cell.bg = DIM_BLUE;
			cell.fg = FAINT;
		} else if cell.bg == CREAM {
			cell.bg = FAINT;
			cell.fg = INK;
		} else {
			cell.bg = INK;
			cell.fg = FAINT;
		}
	}
}

/// A bordered box in the middle of the screen, with a title in its border.
fn frame(
	pen: &mut Pen,
	area: Rect,
	columns: u16,
	rows: u16,
	title: &str,
) -> Rect {
	let columns = columns.min(area.width.saturating_sub(2));
	let rows = rows.min(area.height.saturating_sub(2));
	let inside = Rect::new(
		(area.width - columns) / 2,
		(area.height - rows) / 2,
		columns,
		rows,
	);
	// The box starts from empty cells, so that nothing it covers shows
	// through: neither a character nor its weight.
	for y in inside.y..inside.bottom() {
		for x in inside.x..inside.right() {
			if let Some(cell) = pen.buffer.cell_mut((x, y)) {
				cell.reset();
			}
		}
	}
	pen.fill(inside, Style::new().fg(CREAM).bg(INK));
	pen.rule(inside.x, inside.y, inside.width, "─", text());
	pen.rule(inside.x, inside.bottom() - 1, inside.width, "─", text());
	for y in inside.y..inside.bottom() {
		pen.text(inside.x, y, "│", text(), 1);
		pen.text(inside.right() - 1, y, "│", text(), 1);
	}
	for (x, y, corner) in [
		(inside.x, inside.y, "┌"),
		(inside.right() - 1, inside.y, "┐"),
		(inside.x, inside.bottom() - 1, "└"),
		(inside.right() - 1, inside.bottom() - 1, "┘"),
	] {
		pen.text(x, y, corner, text(), 1);
	}
	pen.text(
		inside.x + 2,
		inside.y,
		&format!(" {title} "),
		strong(),
		inside.width.saturating_sub(4),
	);
	Rect::new(
		inside.x + 3,
		inside.y + 2,
		inside.width.saturating_sub(6),
		inside.height.saturating_sub(3),
	)
}

/// The open box.
fn overlay_box(app: &App, overlay: &Overlay, pen: &mut Pen, area: Rect) {
	match overlay {
		Overlay::Keys => keys_box(pen, area),
		Overlay::All(comparisons, unseen) => {
			all_box(app, comparisons, *unseen, pen, area);
		}
		Overlay::Reviewer(name) => {
			let inside = frame(pen, area, 60, 7, "Reviewer");
			pen.text(
				inside.x,
				inside.y,
				"The name each review recorded from now on will carry.",
				dim(),
				inside.width,
			);
			let end = pen.text(
				inside.x,
				inside.y + 1,
				name,
				strong(),
				inside.width.saturating_sub(1),
			);
			pen.text(end, inside.y + 1, "█", text(), 1);
			pen.keys(
				inside.x,
				inside.y + 3,
				&[("Enter", "keep"), ("Esc", "cancel")],
			);
		}
		Overlay::Contexts(at) => {
			let contexts = app.contexts();
			let inside =
				frame(pen, area, 60, count(contexts.len().min(12)) + 6, "Context");
			for (y, (index, context)) in
				(inside.y..).zip(contexts.iter().enumerate().take(12))
			{
				let on = index == *at;
				if on {
					pen.fill(Rect::new(inside.x, y, inside.width, 1), selected());
				}
				pen.text(
					inside.x + 1,
					y,
					context,
					if on { selected() } else { text() },
					inside.width.saturating_sub(1),
				);
			}
			pen.keys(
				inside.x,
				inside.bottom() - 1,
				&[("Enter", "evaluate under it"), ("Esc", "cancel")],
			);
		}
		Overlay::Projects => {
			let projects = &app.session.state.projects;
			let inside =
				frame(pen, area, 60, count(projects.len().min(12)) + 6, "Projects");
			for (y, project) in (inside.y..).zip(projects.iter().take(12)) {
				pen.text(
					inside.x + 1,
					y,
					&app.workspace().display_path(project),
					text(),
					inside.width.saturating_sub(1),
				);
			}
			pen.keys(inside.x, inside.bottom() - 1, &[("Esc", "close")]);
		}
	}
}

/// A count of rows, as a coordinate.
fn count(rows: usize) -> u16 {
	u16::try_from(rows).unwrap_or(u16::MAX)
}

/// The list of keys.
fn keys_box(pen: &mut Pen, area: Rect) {
	let keys = [
		(
			"↑ ↓",
			"Move between documents; ↑ from the first enters the header",
		),
		("o", "Open or close the other documents"),
		(
			"Tab",
			"Go to the next changed source; Shift+Tab to the previous",
		),
		("v", "Swap the pane between the changes and the document"),
		(
			"PgUp PgDn",
			"Scroll the pane by a page; the wheel by three rows",
		),
		("y", "Answer yes: the document is still accurate"),
		("r", "Answer yes, and record that all of it was reread"),
		("Enter", "Record the answer; Esc cancels it"),
		("a", "Answer yes to every document, after a confirmation"),
		("e", "Edit the document in your editor"),
		("q", "Quit; reviews already recorded stay"),
	];
	let inside = frame(pen, area, 78, count(keys.len()) + 4, "Keys");
	for (y, (key, action)) in (inside.y..).zip(keys) {
		pen.text(inside.x, y, key, strong(), 10);
		pen.text(
			inside.x + 11,
			y,
			action,
			text(),
			inside.width.saturating_sub(11),
		);
	}
}

/// The confirmation before every remaining document is recorded: each one
/// with its outcome, and whether its comparison has been seen.
fn all_box(
	app: &App,
	comparisons: &[(
		super::super::state::Key,
		source_sentry_core::mutate::DocumentDiff,
	)],
	unseen: usize,
	pen: &mut Pen,
	area: Rect,
) {
	let inside = frame(
		pen,
		area,
		76,
		count(comparisons.len().min(10)) + 8,
		"Record all remaining",
	);
	let ask = format!(
		"Record the remaining {}, including {unseen} you have not seen?",
		crate::describe::count(comparisons.len(), "document", "documents")
	);
	pen.text(inside.x, inside.y, &ask, strong(), inside.width);
	for (y, (id, compared)) in (inside.y + 2..).zip(comparisons.iter().take(10)) {
		if let Some(document) = app.session.state.documents.get(id) {
			let label = Style::new()
				.fg(outcome_color(document.outcome()))
				.add_modifier(Modifier::BOLD);
			pen.text(
				inside.x + 2,
				y,
				output::outcome_name(document.outcome()),
				label,
				4,
			);
		}
		let seen = app.session.has_seen(id, compared);
		pen.text(
			inside.x + 8,
			y,
			&fit_path(
				&id.display(app.workspace()),
				inside.width.saturating_sub(20),
			),
			text(),
			inside.width,
		);
		pen.right(
			inside.right(),
			y,
			if seen { "seen" } else { "not seen" },
			if seen {
				dim()
			} else {
				Style::new().fg(outcome_color(Outcome::Warn))
			},
		);
	}
	pen.keys(
		inside.x,
		inside.bottom() - 1,
		&[("y", "record them all"), ("n", "go back, the default")],
	);
}
