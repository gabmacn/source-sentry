//! Pattern sources: globs resolved against a base directory and expanded
//! over tracked paths.

use std::collections::BTreeSet;

use globset::{GlobBuilder, GlobMatcher};

use crate::model::{PathError, RepoPath};

/// A pattern source, resolved against the repository root and compiled.
///
/// The base depends on the spelling: a leading `/` is the repository root,
/// and any other pattern is relative to the directory of the document that
/// declares it, where leading `./` and `../` components apply and may climb
/// no higher than the root. A remote document has no directory, so its
/// patterns must start with `/`.
///
/// Matching uses gitignore-style glob conventions on the whole repository
/// path: `*`, `?` and classes such as `[*]` never match `/`, while `**`
/// matches any number of directories when it is a whole component (elsewhere
/// it acts as `*`). `{a,b}` matches either alternative.
/// Matching is case-sensitive everywhere.
///
/// Discovery expands every live pattern source with it. It is public so that
/// the mutation commands can expand a pattern before recording it,
/// and so that evaluation can compare patterns by their resolved glob.
///
/// # Examples
///
/// ```
/// use std::collections::BTreeSet;
///
/// use source_sentry_core::claims::SourceGlob;
/// use source_sentry_core::model::RepoPath;
///
/// let path = |path| RepoPath::new(path).unwrap();
/// let document = path("docs/guide.md");
/// let glob = SourceGlob::compile(Some(&document), "../src/*.rs").unwrap();
/// assert_eq!(glob.rooted(), "/src/*.rs");
///
/// let tracked = BTreeSet::from([path("src/a.rs"), path("src/deep/b.rs")]);
/// assert_eq!(glob.expand(&tracked), BTreeSet::from([path("src/a.rs")]));
/// assert!(!glob.is_match(&path("src/deep/b.rs")));
/// ```
#[derive(Clone, Debug)]
pub struct SourceGlob {
	/// The resolved pattern, without its leading `/`.
	glob: String,
	/// The literal directories before the first wildcard, under which every
	/// match lies.
	prefix: RepoPath,
	matcher: GlobMatcher,
}

/// Why a pattern source was rejected.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PatternProblem {
	/// The pattern is empty.
	#[error("the pattern is empty")]
	Empty,
	/// The pattern contains a control character.
	#[error("it contains a control character")]
	ControlCharacter,
	/// The pattern uses `\`, which no Source Sentry path may.
	#[error(
		"it contains a backslash; separate components with `/`, and match a \
		 literal wildcard with a class such as `[*]`"
	)]
	Backslash,
	/// A filesystem-absolute, drive-letter or UNC spelling.
	#[error(
		"it is a filesystem path; start it with a single `/` for the repository \
		 root"
	)]
	FilesystemAbsolute,
	/// A remote document's pattern that does not start with `/`.
	#[error(
		"it is relative, but a remote document's patterns must start with `/`"
	)]
	NotRooted,
	/// The pattern climbs above the repository root.
	#[error("it climbs above the repository root")]
	EscapesRepository,
	/// A `..` that follows a wildcard, which cannot be resolved lexically.
	#[error("`..` follows a wildcard; write the directories out instead")]
	ParentAfterWildcard,
	/// The pattern resolves to the repository root itself.
	#[error("it names the repository root rather than files")]
	Root,
	/// The resolved base is not a valid path.
	#[error(transparent)]
	Path(PathError),
	/// The glob syntax is invalid.
	#[error("{0}")]
	Glob(String),
}

/// Whether a component holds a wildcard, an alternation or a class.
fn is_wildcard(component: &str) -> bool {
	component.contains(['*', '?', '[', ']', '{', '}'])
}

impl SourceGlob {
	/// Resolves and compiles `declared`, a pattern that `document` declares,
	/// or that a remote document declares when `document` is `None`.
	///
	/// # Errors
	///
	/// Fails with the [`PatternProblem`] that applies: an empty pattern, a
	/// control character, a backslash, a filesystem-absolute spelling, a
	/// relative pattern of a remote document, a climb above the root, a `..`
	/// after a wildcard, a pattern that names the root, or invalid glob
	/// syntax.
	pub fn compile(
		document: Option<&RepoPath>,
		declared: &str,
	) -> Result<Self, PatternProblem> {
		Self::compile_project(&RepoPath::ROOT, document, declared)
	}

	/// Compiles a source glob with project and repository root spellings.
	///
	/// # Errors
	///
	/// Fails for invalid globs or paths outside the repository.
	pub fn compile_project(
		project: &RepoPath,
		document: Option<&RepoPath>,
		declared: &str,
	) -> Result<Self, PatternProblem> {
		reject_forbidden(declared)?;
		let (base, body) = match (
			declared.strip_prefix(":/"),
			declared.strip_prefix('/'),
			document,
		) {
			(Some(rooted), _, _) => (RepoPath::ROOT, rooted),
			(None, Some(rooted), _) => (project.clone(), rooted),
			(None, None, Some(document)) => {
				(document.parent().unwrap_or(RepoPath::ROOT), declared)
			}
			(None, None, None) => return Err(PatternProblem::NotRooted),
		};
		let mut components: Vec<&str> = base.components().collect();
		for component in body.split('/') {
			match component {
				"" | "." => {}
				".." => match components.last() {
					None => return Err(PatternProblem::EscapesRepository),
					Some(last) if is_wildcard(last) => {
						return Err(PatternProblem::ParentAfterWildcard);
					}
					Some(_) => {
						components.pop();
					}
				},
				name => components.push(name),
			}
		}
		if components.is_empty() {
			return Err(PatternProblem::Root);
		}
		let literal = components
			.iter()
			.take_while(|component| !is_wildcard(component))
			.count();
		let prefix =
			RepoPath::new(&components[..literal].join("/")).map_err(|error| {
				match error {
					PathError::FilesystemAbsolute(_) => {
						PatternProblem::FilesystemAbsolute
					}
					error => PatternProblem::Path(error),
				}
			})?;
		let glob = components.join("/");
		let matcher = GlobBuilder::new(&glob)
			.literal_separator(true)
			.backslash_escape(false)
			.build()
			.map_err(|error| PatternProblem::Glob(error.kind().to_string()))?
			.compile_matcher();
		Ok(Self {
			glob,
			prefix,
			matcher,
		})
	}

	/// The resolved pattern, written with a leading `/`, such as `/src/*.rs`
	/// for `../src/*.rs` declared in `docs/guide.md`. Two patterns with the
	/// same rooted form match the same paths.
	#[must_use]
	pub fn rooted(&self) -> String {
		format!("/{}", self.glob)
	}

	/// Whether the pattern matches `path`, excluding Source Sentry lockfiles.
	#[must_use]
	pub fn is_match(&self, path: &RepoPath) -> bool {
		!path.is_lockfile()
			&& path.starts_with(&self.prefix)
			&& self.matcher.is_match(path.as_str())
	}

	/// Every path in `tracked` that the pattern matches: its current matches,
	/// when `tracked` holds a snapshot's tracked paths.
	#[must_use]
	pub fn expand(&self, tracked: &BTreeSet<RepoPath>) -> BTreeSet<RepoPath> {
		tracked
			.iter()
			.filter(|path| self.is_match(path))
			.cloned()
			.collect()
	}
}

/// Rejects the spellings that no Source Sentry path may use.
fn reject_forbidden(declared: &str) -> Result<(), PatternProblem> {
	if declared.is_empty() {
		return Err(PatternProblem::Empty);
	}
	if declared.chars().any(char::is_control) {
		return Err(PatternProblem::ControlCharacter);
	}
	if declared.contains('\\') {
		return Err(PatternProblem::Backslash);
	}
	let bytes = declared.as_bytes();
	let drive =
		bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
	if drive || declared.starts_with("//") {
		return Err(PatternProblem::FilesystemAbsolute);
	}
	Ok(())
}
