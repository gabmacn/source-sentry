//! The mark with the product's name, version and licence, and a column of
//! facts beside it: the frame shared by a session's opening and closing.
//! The mark is shaded where the terminal shows 24-bit colour.

use std::fmt::Write as _;

use crate::{mark, output};

/// The column where the text starts, clear of the mark.
pub(super) const COLUMN: usize = 28;
/// The line of the mark that carries the product's name and version. The
/// licence follows on the next line.
const TITLE_LINE: usize = 2;
/// The line of the mark that carries the first fact; the others follow it.
pub(super) const FIRST_FACT_LINE: usize = 5;
/// The width of the facts' label column.
const LABEL_WIDTH: usize = 11;

/// The mark with the title, the licence and `facts` beside it, without a
/// final newline. Facts beyond the mark's height continue below it.
pub(super) fn text(facts: &[(&str, String)]) -> String {
	let height = mark::MARK
		.lines()
		.count()
		.max(FIRST_FACT_LINE + facts.len());
	let mut beside = vec![String::new(); height];
	if let Some(row) = beside.get_mut(TITLE_LINE) {
		*row = output::bold(concat!("Source Sentry ", env!("CARGO_PKG_VERSION")));
	}
	if let Some(row) = beside.get_mut(TITLE_LINE + 1) {
		*row = output::dim(concat!(env!("CARGO_PKG_LICENSE"), " licence"));
	}
	for (row, (label, value)) in
		beside.iter_mut().skip(FIRST_FACT_LINE).zip(facts)
	{
		*row = format!(
			"{}{value}",
			output::pad(&output::dim(label), label, LABEL_WIDTH)
		);
	}
	// The plain mark gives each line's width; the dressed one is what is
	// written, and its colours take no room.
	let dressed = mark::dressed(output::full_colour());
	let mut drawn = dressed.lines().zip(mark::MARK.lines());
	let mut text = String::new();
	for row in &beside {
		let (line, plain) = drawn.next().unwrap_or_default();
		if !text.is_empty() {
			text.push('\n');
		}
		text.push_str(line);
		if !row.is_empty() {
			let gap = COLUMN.saturating_sub(plain.chars().count());
			let _ = write!(text, "{:gap$}{row}", "");
		}
	}
	text
}

#[cfg(test)]
mod tests {
	use super::{COLUMN, text};
	use crate::mark;

	/// The title, the licence and the facts share one column beside an intact
	/// mark, and facts past the mark's height keep that column.
	#[test]
	fn the_text_lines_up_beside_the_mark() {
		let facts: Vec<_> = (0..9)
			.map(|index| ("Label", format!("value {index}")))
			.collect();
		let shown = anstream::adapter::strip_str(&text(&facts)).to_string();
		let lines: Vec<_> = shown.lines().collect();
		assert_eq!(lines.len(), 14);
		let beside = |line: &str| line.chars().skip(COLUMN).collect::<String>();
		assert_eq!(
			beside(lines[2]),
			concat!("Source Sentry ", env!("CARGO_PKG_VERSION"))
		);
		assert_eq!(
			beside(lines[3]),
			concat!(env!("CARGO_PKG_LICENSE"), " licence")
		);
		assert_eq!(beside(lines[4]), "");
		assert_eq!(beside(lines[5]), "Label      value 0");
		assert_eq!(beside(lines[13]), "Label      value 8");
		for (line, drawn) in lines.iter().zip(mark::MARK.lines()) {
			let left: String = line.chars().take(COLUMN).collect();
			assert_eq!(left.trim_end(), drawn);
		}
	}
}
