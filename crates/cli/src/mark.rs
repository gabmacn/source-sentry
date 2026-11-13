//! The Source Sentry mark for terminals: an eye ringed by six feather tips.
//!
//! Each character holds two pixels, one above the other, so the mark is drawn
//! with the half-block characters alone. It carries no colour of its own and
//! takes the terminal's foreground, which suits light and dark terminals
//! alike.

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

#[cfg(test)]
mod tests {
	use super::{MARK, SMALL};

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
