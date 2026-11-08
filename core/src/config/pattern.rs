//! Path patterns, which are gitignore-style globs, and series patterns, which
//! are regular expressions.

use std::fmt;

use globset::{GlobBuilder, GlobMatcher};
use regex::Regex;

use crate::model::RepoPath;

/// Why a pattern was rejected, and how to fix it when that is known.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PatternError {
	/// What is wrong with the pattern.
	pub(super) reason: String,
	/// A concrete correction.
	pub(super) correction: Option<String>,
}

impl PatternError {
	fn new(reason: impl Into<String>) -> Self {
		Self {
			reason: reason.into(),
			correction: None,
		}
	}

	fn correct(mut self, correction: impl Into<String>) -> Self {
		self.correction = Some(correction.into());
		self
	}
}

/// A gitignore-style glob, anchored at a directory.
///
/// The syntax is `.gitignore`'s: `*` and `?` never match `/`, `**` matches any
/// number of directories, a trailing `/` matches directories only, and a
/// pattern with a `/` at its start or in its middle is relative to the anchor,
/// while any other pattern matches at any depth below it. A leading `!` marks
/// an exception, which only ignore patterns may use, and `\!` or `\#` start a
/// pattern with a literal `!` or `#`. Matching is case-sensitive on every
/// platform, so that results never depend on the machine.
///
/// The anchor is the directory of the config that consumes the pattern: the
/// repository root for the repository config and the built-in defaults, the
/// config's directory for a directory config, and the importing config's
/// directory for a preset.
#[derive(Clone, Debug)]
pub struct PathPattern {
	source: String,
	anchor: RepoPath,
	negated: bool,
	directory_only: bool,
	matcher: GlobMatcher,
}

impl PathPattern {
	/// Compiles a pattern anchored at the repository root. Only ignore
	/// patterns pass `allow_negation`.
	pub(super) fn compile(
		source: &str,
		allow_negation: bool,
	) -> Result<Self, PatternError> {
		if source.trim().is_empty() {
			return Err(PatternError::new("the pattern is empty"));
		}
		let mut rest = source;
		let mut negated = false;
		if rest.starts_with("\\!") || rest.starts_with("\\#") {
			rest = &rest[1..];
		} else if let Some(exception) = rest.strip_prefix('!') {
			if !allow_negation {
				return Err(
					PatternError::new("only ignore patterns may start with `!`").correct(
						format!(
							"To exclude these paths, add {} to `documents.ignore`",
							quote(exception)
						),
					),
				);
			}
			negated = true;
			rest = exception;
		} else if rest.starts_with('#') {
			return Err(
				PatternError::new("a leading `#` starts a comment in .gitignore")
					.correct(format!(
						"Use {} to match a literal `#`",
						quote(&format!("\\{rest}"))
					)),
			);
		}
		// As in .gitignore, trailing spaces are ignored unless escaped.
		if !rest.ends_with("\\ ") {
			rest = rest.trim_end_matches(' ');
		}
		let anchored = rest.starts_with('/');
		if anchored {
			rest = &rest[1..];
		}
		let directory_only = rest.ends_with('/');
		if directory_only {
			rest = &rest[..rest.len() - 1];
			// An escaped trailing slash is still a slash (as in ripgrep #2236).
			rest = rest.strip_suffix('\\').unwrap_or(rest);
		}
		if rest.is_empty() {
			return Err(PatternError::new("the pattern matches no path"));
		}
		let mut glob = rest.to_owned();
		if !anchored && !rest.contains('/') && !rest.starts_with("**/") {
			glob = format!("**/{glob}");
		}
		// `dir/**` matches what is inside `dir`, but not `dir` itself.
		if glob.ends_with("/**") {
			glob.push_str("/*");
		}
		let matcher = GlobBuilder::new(&glob)
			.literal_separator(true)
			.backslash_escape(true)
			.build()
			.map_err(|error| PatternError::new(error.kind().to_string()))?
			.compile_matcher();
		Ok(Self {
			source: source.to_owned(),
			anchor: RepoPath::ROOT,
			negated,
			directory_only,
			matcher,
		})
	}

	/// The same pattern, anchored at `anchor`.
	pub(super) fn anchored_at(&self, anchor: &RepoPath) -> Self {
		Self {
			anchor: anchor.clone(),
			..self.clone()
		}
	}

	/// The pattern as written, including any leading `!`.
	#[must_use]
	pub fn as_str(&self) -> &str {
		&self.source
	}

	/// The directory the pattern is relative to.
	#[must_use]
	pub fn anchor(&self) -> &RepoPath {
		&self.anchor
	}

	/// Whether the pattern is an exception, written with a leading `!`.
	#[must_use]
	pub fn is_negated(&self) -> bool {
		self.negated
	}

	/// Whether the pattern matches `path`, a file, or one of the directories
	/// that contain it below the anchor. A directory match covers everything
	/// inside the directory, as in `.gitignore`. Paths outside the anchor
	/// never match.
	///
	/// Negation does not change the answer: an exception "matches" the paths
	/// it brings back.
	#[must_use]
	pub fn matches(&self, path: &RepoPath) -> bool {
		let Some(relative) = path.strip_prefix(&self.anchor) else {
			return false;
		};
		if relative.is_empty() {
			return false;
		}
		if !self.directory_only && self.matcher.is_match(relative) {
			return true;
		}
		relative
			.match_indices('/')
			.any(|(end, _)| self.matcher.is_match(&relative[..end]))
	}
}

impl PartialEq for PathPattern {
	fn eq(&self, other: &Self) -> bool {
		self.source == other.source && self.anchor == other.anchor
	}
}

impl Eq for PathPattern {}

impl fmt::Display for PathPattern {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		formatter.write_str(&self.source)
	}
}

/// A series pattern: a regular expression with a capture group named `version`.
///
/// It is implicitly anchored: it must match a whole path relative to the
/// directory of the config that defines or imports the series.
#[derive(Clone, Debug)]
pub struct SeriesPattern {
	source: String,
	regex: Regex,
}

impl SeriesPattern {
	/// The name of the group that captures the version.
	pub const VERSION_GROUP: &str = "version";

	/// Compiles a pattern, anchored at both ends.
	pub(super) fn compile(source: &str) -> Result<Self, PatternError> {
		// The pattern on its own first, so that errors quote what was written.
		Regex::new(source).map_err(|error| PatternError::new(error.to_string()))?;
		let regex = Regex::new(&format!("^(?:{source})$"))
			.map_err(|error| PatternError::new(error.to_string()))?;
		let has_version = regex
			.capture_names()
			.flatten()
			.any(|name| name == Self::VERSION_GROUP);
		if !has_version {
			return Err(
				PatternError::new("it has no capture group named `version`").correct(
					"Capture the version with `(?P<version>...)`, as in \
					 'audit-(?P<version>\\d+)\\.md'",
				),
			);
		}
		Ok(Self {
			source: source.to_owned(),
			regex,
		})
	}

	/// The pattern as written.
	#[must_use]
	pub fn as_str(&self) -> &str {
		&self.source
	}

	/// The version that `relative` carries, when the whole of `relative`
	/// matches. `None` when it does not match, or when the `version` group
	/// did not take part in the match.
	#[must_use]
	pub fn version<'p>(&self, relative: &'p str) -> Option<&'p str> {
		self
			.regex
			.captures(relative)?
			.name(Self::VERSION_GROUP)
			.map(|version| version.as_str())
	}
}

impl PartialEq for SeriesPattern {
	fn eq(&self, other: &Self) -> bool {
		self.source == other.source
	}
}

impl Eq for SeriesPattern {}

impl fmt::Display for SeriesPattern {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		formatter.write_str(&self.source)
	}
}

/// Writes `value` as a single-quoted YAML scalar, the convention for globs and
/// regular expressions.
pub(super) fn quote(value: &str) -> String {
	format!("'{}'", value.replace('\'', "''"))
}
