//! The Source Sentry mark for terminals: an eye ringed by six feather tips.
//!
//! Each character holds two pixels, one above the other, so the mark is drawn
//! with the half-block characters alone. As text it carries no colour of its
//! own and takes the terminal's foreground, which suits light and dark
//! terminals alike.
//!
//! Where a terminal shows 24-bit colour, [`dressed`] shades the mark with a
//! gradient: each character takes the colour of its place. Only the
//! foreground is set, so the shaded mark with its colours stripped is the
//! plain mark again, character for character.

use std::borrow::Cow;
use std::fmt::Write as _;

/// The mark, at most 24 columns wide and 12 lines tall, without a final
/// newline. It uses only `▀`, `▄` and `█`, which belong to the character set
/// of the first IBM PC, so legacy console fonts carry them too.
pub(crate) const MARK: &str = concat!(
	"    ▄▄▄   ▄██▄   ▄▄▄\n",
	"    ███   ▀██▀   ███\n",
	"      ▀█   ▀▀   █▀\n",
	"       ▄▄██████▄▄\n",
	"    ▄▄█▀▀▄▄██▄▄▀▀█▄▄\n",
	"  ▄██▀  ▄███▄ █▄  ▀██▄\n",
	"  ▀██▄  ▀██████▀  ▄██▀\n",
	"    ▀▀█▄▄▀▀██▀▀▄▄█▀▀\n",
	"       ▀▀██████▀▀\n",
	"      ▄█   ▄▄   █▄\n",
	"    ███   ▄██▄   ███\n",
	"    ▀▀▀   ▀██▀   ▀▀▀",
);

/// The mark where the larger one has no room: at most 16 columns wide and 8
/// lines tall, in the same three characters.
pub(crate) const SMALL: &str = concat!(
	"   ▄▄  ██  ▄▄\n",
	"   ▀█▄ ▀▀ ▄█▀\n",
	"    ▄▄▀▀▀▀▄▄\n",
	"  ▄▀▀ ██▀█ ▀▀▄\n",
	"  ▀▄▄ ████ ▄▄▀\n",
	"    ▀▀▄▄▄▄▀▀\n",
	"   ▄█▀ ▄▄ ▀█▄\n",
	"   ▀▀  ██  ▀▀",
);

/// A colour, as red, green and blue.
pub(crate) type Rgb = (u8, u8, u8);

/// How a gradient runs across the mark.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Run {
	/// From the centre of the mark outwards.
	Radial,
	/// Along this direction, as columns across and pixel rows down: `(1, 0)`
	/// runs from left to right, `(0, 1)` from top to bottom, and `(1, 1)`
	/// from the top left to the bottom right.
	#[cfg_attr(
		not(test),
		expect(
			dead_code,
			reason = "the mark's own gradient is radial; a direction is kept for trying others"
		)
	)]
	Along(i32, i32),
}

/// A gradient: the colours it passes through, in order, and how it runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Gradient {
	/// The colours, from where the gradient starts to where it ends.
	pub(crate) stops: &'static [Rgb],
	/// How it runs across the mark.
	pub(crate) run: Run,
}

/// The gradient of the mark in the command's own output. Its colours stay in
/// the middle range of lightness, because one gradient has to read on dark
/// and on light terminals alike: each keeps a contrast of at least 3 to 1
/// against a near-black and a near-white ground.
pub(crate) const GRADIENT: Gradient = Gradient {
	stops: &[(0x7a, 0x85, 0xff), (0x55, 0x60, 0xff), (0x3f, 0x4a, 0xff)],
	run: Run::Radial,
};

/// The mark for the command's own output: shaded with [`GRADIENT`] where
/// `colour` says that the terminal shows 24-bit colour, and plain otherwise.
pub(crate) fn dressed(colour: bool) -> Cow<'static, str> {
	if colour {
		Cow::Owned(shaded(MARK, &GRADIENT))
	} else {
		Cow::Borrowed(MARK)
	}
}

/// The place of a character's ink, in half pixels: the upper pixel of `▀`,
/// the lower one of `▄`, and the middle of `█`. A pixel is as tall as it is
/// wide, because a character is twice as tall as wide.
fn place(column: usize, row: usize, symbol: char) -> Option<(i32, i32)> {
	let within = match symbol {
		'▀' => 1,
		'█' => 2,
		'▄' => 3,
		_ => return None,
	};
	let column = i32::try_from(column).ok()?;
	let row = i32::try_from(row).ok()?;
	Some((2 * column + 1, 4 * row + within))
}

/// `mark` with each character in the colour of its place along `gradient`,
/// as 24-bit foreground colours. The gradient spans the mark's own extent:
/// its first colour falls on the nearest ink and its last on the farthest.
pub(crate) fn shaded(mark: &str, gradient: &Gradient) -> String {
	let cells = |row: usize, line: &str| {
		line
			.chars()
			.enumerate()
			.filter_map(move |(column, symbol)| place(column, row, symbol))
			.collect::<Vec<_>>()
	};
	let inked: Vec<(i32, i32)> = mark
		.lines()
		.enumerate()
		.flat_map(|(row, line)| cells(row, line))
		.collect();
	let span = |values: &mut dyn Iterator<Item = i32>| {
		values.fold((i32::MAX, i32::MIN), |(low, high), value| {
			(low.min(value), high.max(value))
		})
	};
	let (left, right) = span(&mut inked.iter().map(|(x, _)| *x));
	let (top, bottom) = span(&mut inked.iter().map(|(_, y)| *y));
	// How far along the gradient a place lies, before it is scaled.
	let along = |(x, y): (i32, i32)| {
		let (x, y) = (2 * x - left - right, 2 * y - top - bottom);
		match gradient.run {
			Run::Radial => {
				let squared = u32::try_from(x * x + y * y).unwrap_or(0);
				i32::try_from((squared << 8).isqrt()).unwrap_or(i32::MAX)
			}
			Run::Along(across, down) => x * across + y * down,
		}
	};
	let (nearest, farthest) = span(&mut inked.iter().map(|place| along(*place)));
	let mut text = String::new();
	for (row, line) in mark.lines().enumerate() {
		if row > 0 {
			text.push('\n');
		}
		let mut current = None;
		for (column, symbol) in line.chars().enumerate() {
			if let Some(place) = place(column, row, symbol) {
				let colour =
					colour_at(gradient.stops, along(place) - nearest, farthest - nearest);
				if colour != current {
					if let Some((red, green, blue)) = colour {
						let _ = write!(text, "\u{1b}[38;2;{red};{green};{blue}m");
					}
					current = colour;
				}
			}
			text.push(symbol);
		}
		if current.is_some() {
			text.push_str("\u{1b}[39m");
		}
	}
	text
}

/// The colour `part` of the way through `whole` along `stops`, mixed between
/// the two stops it lies between; `None` without stops.
fn colour_at(stops: &[Rgb], part: i32, whole: i32) -> Option<Rgb> {
	let last = stops.len().checked_sub(1)?;
	if last == 0 || whole <= 0 {
		return stops.first().copied();
	}
	// The position in 1024ths of a step between two neighbouring stops.
	let between = u64::try_from(last).ok()?;
	let scaled = u64::try_from(part.clamp(0, whole)).ok()? * between * 1024
		/ u64::try_from(whole).ok()?;
	let index = usize::try_from(scaled / 1024).ok()?.min(last - 1);
	let share = scaled - u64::try_from(index).ok()? * 1024;
	let (from, to) = (stops[index], stops[index + 1]);
	let mix = |from: u8, to: u8| {
		let mixed =
			(u64::from(from) * (1024 - share) + u64::from(to) * share + 512) / 1024;
		u8::try_from(mixed).unwrap_or(u8::MAX)
	};
	Some((mix(from.0, to.0), mix(from.1, to.1), mix(from.2, to.2)))
}

#[cfg(test)]
mod tests {
	use super::{
		GRADIENT, Gradient, MARK, Run, SMALL, colour_at, dressed, shaded,
	};

	/// Shading sets foreground colours and nothing else: without them the
	/// shaded mark is the plain one, so a stream that strips colour, or a
	/// terminal that is given none, shows the mark intact.
	#[test]
	fn a_shaded_mark_without_its_colours_is_the_plain_mark() {
		for mark in [MARK, SMALL] {
			let drawn = shaded(mark, &GRADIENT);
			assert!(drawn.contains("\u{1b}[38;2;"));
			assert_eq!(anstream::adapter::strip_str(&drawn).to_string(), mark);
			// Every line that takes a colour gives the foreground back.
			for line in drawn.lines() {
				assert!(line.ends_with("\u{1b}[39m"), "{line:?}");
			}
		}
		assert_eq!(dressed(false), MARK);
		assert_eq!(dressed(true), shaded(MARK, &GRADIENT));
	}

	/// A gradient spans the mark: its first colour falls on the nearest ink
	/// and its last on the farthest, with mixtures between.
	#[test]
	fn a_gradient_runs_from_its_first_colour_to_its_last() {
		let stops = &[(0, 0, 0), (100, 200, 40), (255, 255, 255)];
		assert_eq!(colour_at(stops, 0, 10), Some((0, 0, 0)));
		assert_eq!(colour_at(stops, 5, 10), Some((100, 200, 40)));
		assert_eq!(colour_at(stops, 10, 10), Some((255, 255, 255)));
		assert_eq!(colour_at(stops, 25, 100), Some((50, 100, 20)));
		assert_eq!(colour_at(stops, 99, 10), Some((255, 255, 255)));
		assert_eq!(colour_at(&stops[..1], 7, 10), Some((0, 0, 0)));
		assert_eq!(colour_at(&[], 7, 10), None);

		// From left to right, the first column is the first colour and the
		// last column the last one.
		let across = Gradient {
			stops: &[(10, 20, 30), (200, 100, 50)],
			run: Run::Along(1, 0),
		};
		let drawn = shaded("█ █\n███", &across);
		assert_eq!(
			drawn,
			"\u{1b}[38;2;10;20;30m█ \u{1b}[38;2;200;100;50m█\u{1b}[39m\n\
			 \u{1b}[38;2;10;20;30m█\u{1b}[38;2;105;60;40m█\
			 \u{1b}[38;2;200;100;50m█\u{1b}[39m"
		);
		// From the centre outwards, the middle differs from the corners, and
		// the corners agree with one another.
		let radial = Gradient {
			stops: &[(255, 0, 0), (0, 0, 255)],
			run: Run::Radial,
		};
		let drawn = shaded("███\n███\n███", &radial);
		let lines: Vec<_> = drawn.lines().collect();
		assert!(lines[1].contains("\u{1b}[38;2;255;0;0m█"), "{drawn:?}");
		assert!(lines[0].starts_with("\u{1b}[38;2;0;0;255m█"), "{drawn:?}");
		assert!(lines[2].starts_with("\u{1b}[38;2;0;0;255m█"), "{drawn:?}");
	}

	/// The smaller mark keeps its size and the same three characters.
	#[test]
	fn the_smaller_mark_fits_16_columns_and_uses_half_blocks_alone() {
		let lines: Vec<_> = SMALL.lines().collect();
		assert_eq!(lines.len(), 8);
		for line in lines {
			assert!(line.chars().count() <= 16, "{line:?}");
			assert_eq!(line, line.trim_end(), "no trailing spaces");
			assert!(
				line.chars().all(|c| matches!(c, ' ' | '▀' | '▄' | '█')),
				"{line:?}"
			);
		}
	}

	/// The mark keeps the size and the three characters that its callers and
	/// legacy consoles rely on.
	#[test]
	fn the_mark_fits_24_columns_and_uses_half_blocks_alone() {
		let lines: Vec<_> = MARK.lines().collect();
		assert_eq!(lines.len(), 12);
		for line in lines {
			assert!(line.chars().count() <= 24, "{line:?}");
			assert_eq!(line, line.trim_end(), "no trailing spaces");
			assert!(
				line.chars().all(|c| matches!(c, ' ' | '▀' | '▄' | '█')),
				"{line:?}"
			);
		}
	}
}
