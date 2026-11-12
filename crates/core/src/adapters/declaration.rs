//! What frontmatter and lockfiles share: reading a declaration with precise
//! errors, and validating where each field may appear.

use std::{collections::BTreeMap, fmt};

use serde::{Deserialize, Deserializer, de::Error as _};

use crate::model::{
	ClassName, Declaration, Member, ObjectId, PathError, PathSource,
	PatternSource, Rename, RepoPath, Retired, Review, Schedule, Seal, SourceDecl,
	resolve_project_source,
};

/// The kind of file a declaration was read from or is written to, which
/// decides the fields it may carry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Site {
	/// A document's own frontmatter.
	Frontmatter,
	/// A `source-sentry.lock` beside the documents.
	LocalLockfile,
	/// An owner lockfile in `.source-sentry/remotes/`.
	RemoteLockfile,
}

impl fmt::Display for Site {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		formatter.write_str(match self {
			Self::Frontmatter => "in frontmatter",
			Self::LocalLockfile => "in a local lockfile",
			Self::RemoteLockfile => "in a remote lockfile",
		})
	}
}

/// A declaration that parsed, but breaks a rule of the record. Each error
/// names the offending field as a path such as
/// `source-sentry.sources[1].path` or `documents["a.md"].seal`.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DeclarationError {
	/// A former document path is not a canonical rooted spelling.
	#[error("`{at}`: {reason}")]
	InvalidRename {
		/// The former path's field.
		at: String,
		/// The constraint it violates.
		reason: &'static str,
	},
	/// A field that this kind of file must not carry: `seal` outside local
	/// lockfiles, `url` outside remote lockfiles, or `completed` in a remote
	/// lockfile.
	#[error("`{at}` is not allowed {site}: {reason}")]
	NotAllowed {
		/// The field.
		at: String,
		/// Where it was found.
		site: Site,
		/// Why it is not allowed there.
		reason: &'static str,
	},
	/// A path source with `oid` but no `reviewed`, or the reverse.
	#[error(
		"`{at}` has `{present}` but no `{missing}`; an attested source carries \
		 both, and an unattested one neither"
	)]
	Unpaired {
		/// The source.
		at: String,
		/// The field that is present.
		present: &'static str,
		/// The field that is missing.
		missing: &'static str,
	},
	/// A source or member path that is not resolvable, such as a
	/// filesystem-absolute path or one that climbs above the repository root,
	/// or a remote document's source that is not `/`-rooted.
	#[error("`{at}`: {error}")]
	Path {
		/// The path or pattern field.
		at: String,
		/// Why it was rejected.
		error: PathError,
	},
	/// An owner that is empty or only whitespace, which a directory config
	/// rejects as well.
	#[error("`{at}` is blank; an owner is a non-blank identifier")]
	BlankOwner {
		/// The owner's field.
		at: String,
	},
	/// Two sources of one document that name the same file, or the same
	/// pattern, or two members of one pattern that name the same file.
	#[error("`{at}` repeats `{first}`: both name `{target}`")]
	Duplicate {
		/// The later field.
		at: String,
		/// The earlier field.
		first: String,
		/// The resolved path, or the pattern, that both name.
		target: String,
	},
}

/// A location inside a parsed file, rendered like
/// `documents["a.md"].sources[0].oid`. Keys in lowercase kebab-case are
/// written after a dot; any other key is quoted in brackets.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct FieldPath(String);

impl FieldPath {
	/// The path of a top-level key.
	pub(super) fn root(key: &str) -> Self {
		Self::default().key(key)
	}

	/// The path of `key` inside this one.
	pub(super) fn key(&self, key: &str) -> Self {
		let plain = key
			.bytes()
			.next()
			.is_some_and(|byte| byte.is_ascii_lowercase())
			&& key.bytes().all(|byte| {
				byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'
			});
		Self(match (plain, self.0.is_empty()) {
			(true, true) => key.to_owned(),
			(true, false) => format!("{}.{key}", self.0),
			(false, _) => format!("{}[{key:?}]", self.0),
		})
	}

	/// The path of the entry `key` of a lockfile section, which is always
	/// quoted, so that a document named `sources` does not read as a field.
	pub(super) fn entry(&self, key: &str) -> Self {
		Self(format!("{}[{key:?}]", self.0))
	}

	/// The path of the element at `index` inside this one.
	pub(super) fn index(&self, index: usize) -> Self {
		Self(format!("{}[{index}]", self.0))
	}

	/// Whether this is a lockfile section, whose keys are entries.
	pub(super) fn is_section(&self) -> bool {
		self.0 == "documents" || self.0 == "retired"
	}

	/// The path that `serde_path_to_error` recorded.
	pub(super) fn from_serde(path: &serde_path_to_error::Path) -> Self {
		use serde_path_to_error::Segment;

		path
			.iter()
			.fold(Self::default(), |at, segment| match segment {
				Segment::Seq { index } => at.index(*index),
				Segment::Map { key } if at.is_section() => at.entry(key),
				Segment::Map { key } => at.key(key),
				Segment::Enum { variant } => at.key(variant),
				Segment::Unknown => Self(format!("{at}[?]")),
			})
	}

	/// The rendered path.
	pub(super) fn into_string(self) -> String {
		self.0
	}
}

impl fmt::Display for FieldPath {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		formatter.write_str(&self.0)
	}
}

/// A declaration as written in either format.
///
/// It mirrors [`Declaration`] field for field, except that each source is read
/// through [`Source`], so that a malformed source names its problem instead of
/// failing to match the model's untagged enum.
#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(super) struct RawDeclaration {
	#[serde(default)]
	classes: Vec<ClassName>,
	#[serde(default)]
	completed: Option<Review>,
	#[serde(default)]
	owners: Vec<String>,
	#[serde(default)]
	renamed: Vec<Rename>,
	#[serde(default)]
	reviewed: Option<Review>,
	#[serde(default)]
	schedule: Option<Schedule>,
	#[serde(default)]
	seal: Option<Seal>,
	#[serde(default)]
	sources: Vec<Source>,
	#[serde(default)]
	url: Option<String>,
}

impl From<RawDeclaration> for Declaration {
	fn from(raw: RawDeclaration) -> Self {
		Self {
			classes: raw.classes,
			completed: raw.completed,
			owners: raw.owners,
			renamed: raw.renamed,
			reviewed: raw.reviewed,
			schedule: raw.schedule,
			seal: raw.seal,
			sources: raw.sources.into_iter().map(|source| source.0).collect(),
			url: raw.url,
		}
	}
}

/// One source as written. A `pattern` key makes it a pattern source;
/// otherwise it is a path source. It accepts what the model's untagged
/// [`SourceDecl`] accepts, but names the field at fault.
struct Source(SourceDecl);

impl<'de> Deserialize<'de> for Source {
	fn deserialize<D: Deserializer<'de>>(
		deserializer: D,
	) -> Result<Self, D::Error> {
		#[derive(Deserialize)]
		#[serde(rename_all = "kebab-case", deny_unknown_fields)]
		struct Fields {
			members: Option<Vec<Member>>,
			oid: Option<ObjectId>,
			path: Option<String>,
			pattern: Option<String>,
			reviewed: Option<Review>,
		}

		let fields = Fields::deserialize(deserializer)?;
		let source = match (fields.path, fields.pattern) {
			(Some(_), Some(_)) => {
				return Err(D::Error::custom(
					"a source has `path` or `pattern`, not both",
				));
			}
			(None, None) => {
				return Err(D::Error::custom("a source needs `path` or `pattern`"));
			}
			(Some(path), None) => {
				if fields.members.is_some() {
					return Err(D::Error::custom(
						"only a pattern source has `members`; a path source has \
						 `oid` and `reviewed`",
					));
				}
				SourceDecl::Path(PathSource {
					oid: fields.oid,
					path,
					reviewed: fields.reviewed,
				})
			}
			(None, Some(pattern)) => {
				if fields.oid.is_some() {
					return Err(D::Error::custom(
						"a pattern source has no `oid` of its own; \
						 each of its `members` has one",
					));
				}
				SourceDecl::Pattern(PatternSource {
					members: fields.members.unwrap_or_default(),
					pattern,
					reviewed: fields.reviewed,
				})
			}
		};
		Ok(Self(source))
	}
}

/// Where a declaration lives, with what its paths resolve against.
#[derive(Clone, Copy, Debug)]
pub(super) enum Scope<'a> {
	/// In the frontmatter of this document.
	Frontmatter(&'a RepoPath, &'a RepoPath),
	/// In a local lockfile, for this document.
	Local(&'a RepoPath, &'a RepoPath),
	/// In a remote lockfile, where the document has no path.
	Remote(&'a RepoPath),
}

impl Scope<'_> {
	fn site(self) -> Site {
		match self {
			Self::Frontmatter(..) => Site::Frontmatter,
			Self::Local(..) => Site::LocalLockfile,
			Self::Remote(_) => Site::RemoteLockfile,
		}
	}

	/// Resolves a declared source or member path.
	fn resolve(self, declared: &str) -> Result<RepoPath, PathError> {
		match self {
			Self::Frontmatter(project, document) | Self::Local(project, document) => {
				resolve_project_source(project, Some(document), declared)
			}
			Self::Remote(project) => resolve_project_source(project, None, declared),
		}
	}

	/// Checks the part of a pattern that the record constrains: a remote
	/// document's patterns must be `/`-rooted. Everything else about a glob is
	/// left to the code that compiles it.
	fn check_pattern(self, pattern: &str) -> Result<(), PathError> {
		match self {
			Self::Remote(_)
				if !pattern.starts_with('/') && !pattern.starts_with(":/") =>
			{
				Err(PathError::NotRooted(pattern.to_owned()))
			}
			_ => Ok(()),
		}
	}
}

/// Checks that `declaration` may appear where `scope` says, and that each of
/// its sources is well formed. `at` is the path of the declaration itself.
///
/// The checks:
///
/// - `seal` only in a local lockfile, `url` only in a remote one, and
///   `completed` anywhere but a remote one;
/// - each path source has `oid` and `reviewed` together, or neither;
/// - each source and member path resolves within the repository, or, for a
///   remote document, is `/`-rooted, as its patterns are as well;
/// - no two path sources resolve to the same file, no pattern is declared
///   twice, and no two members of one pattern resolve to the same file.
pub(super) fn validate(
	declaration: &Declaration,
	scope: Scope<'_>,
	at: &FieldPath,
) -> Result<(), DeclarationError> {
	let site = scope.site();
	let not_allowed = |field: &str, reason| DeclarationError::NotAllowed {
		at: at.key(field).into_string(),
		site,
		reason,
	};
	match (scope, &declaration.seal) {
		(Scope::Frontmatter(..), Some(_)) => {
			return Err(not_allowed(
				"seal",
				"records are tracked through lockfiles only",
			));
		}
		(Scope::Remote(_), Some(_)) => {
			return Err(not_allowed("seal", "remote documents cannot be records"));
		}
		_ => {}
	}
	if declaration.url.is_some() && !matches!(scope, Scope::Remote(_)) {
		return Err(not_allowed("url", "only remote documents carry a `url`"));
	}
	if declaration.completed.is_some() && matches!(scope, Scope::Remote(_)) {
		return Err(not_allowed(
			"completed",
			"remote documents cannot belong to a series",
		));
	}
	for (index, rename) in declaration.renamed.iter().enumerate() {
		let field = at.key("renamed").index(index).key("from");
		let invalid = |reason| DeclarationError::InvalidRename {
			at: field.to_string(),
			reason,
		};
		let path = rename
			.from
			.strip_prefix(":/")
			.or_else(|| rename.from.strip_prefix('/'))
			.ok_or_else(|| {
				invalid("a former document path must start with `/` or `:/`")
			})?;
		if path.is_empty()
			|| path.split('/').any(|part| matches!(part, "" | "." | ".."))
		{
			return Err(invalid(
				"a former document path must name a document with canonical path components",
			));
		}
		let resolved = resolve(scope, &rename.from, &field)?;
		if resolved.is_lockfile() {
			return Err(invalid(
				"a former document path cannot name a Source Sentry lockfile",
			));
		}
	}
	if let Some(index) = declaration
		.owners
		.iter()
		.position(|owner| owner.trim().is_empty())
	{
		return Err(DeclarationError::BlankOwner {
			at: at.key("owners").index(index).into_string(),
		});
	}
	validate_sources(&declaration.sources, scope, &at.key("sources"))
}

/// The source checks of [`validate`].
fn validate_sources(
	sources: &[SourceDecl],
	scope: Scope<'_>,
	at: &FieldPath,
) -> Result<(), DeclarationError> {
	let mut paths = Seen::default();
	let mut patterns = Seen::default();
	for (index, source) in sources.iter().enumerate() {
		let here = at.index(index);
		match source {
			SourceDecl::Path(source) => {
				let unpaired = match (&source.oid, &source.reviewed) {
					(Some(_), None) => Some(("oid", "reviewed")),
					(None, Some(_)) => Some(("reviewed", "oid")),
					_ => None,
				};
				if let Some((present, missing)) = unpaired {
					return Err(DeclarationError::Unpaired {
						at: here.into_string(),
						present,
						missing,
					});
				}
				let field = here.key("path");
				let target = resolve(scope, &source.path, &field)?;
				paths.insert(target.as_str(), field)?;
			}
			SourceDecl::Pattern(source) => {
				let field = here.key("pattern");
				scope.check_pattern(&source.pattern).map_err(|error| {
					DeclarationError::Path {
						at: field.to_string(),
						error,
					}
				})?;
				patterns.insert(&source.pattern, field)?;
				let mut members = Seen::default();
				let members_at = here.key("members");
				for (index, member) in source.members.iter().enumerate() {
					let field = members_at.index(index).key("path");
					let target = resolve(scope, &member.path, &field)?;
					members.insert(target.as_str(), field)?;
				}
			}
		}
	}
	Ok(())
}

/// Checks that a tombstone may appear where `scope` says: a remote document's
/// tombstone carries no seal. `at` is the path of the tombstone itself.
pub(super) fn validate_retired(
	retired: &Retired,
	scope: Scope<'_>,
	at: &FieldPath,
) -> Result<(), DeclarationError> {
	if retired.seal.is_some() && matches!(scope, Scope::Remote(_)) {
		return Err(DeclarationError::NotAllowed {
			at: at.key("seal").into_string(),
			site: Site::RemoteLockfile,
			reason: "remote documents cannot be records",
		});
	}
	if retired.completed.is_some() && matches!(scope, Scope::Remote(_)) {
		return Err(DeclarationError::NotAllowed {
			at: at.key("completed").into_string(),
			site: Site::RemoteLockfile,
			reason: "remote documents cannot be part of a series",
		});
	}
	Ok(())
}

/// Resolves a declared path, naming `field` if it is rejected.
fn resolve(
	scope: Scope<'_>,
	declared: &str,
	field: &FieldPath,
) -> Result<RepoPath, DeclarationError> {
	scope
		.resolve(declared)
		.map_err(|error| DeclarationError::Path {
			at: field.to_string(),
			error,
		})
}

/// The targets seen so far, each with the field that first named it.
#[derive(Default)]
struct Seen(BTreeMap<String, FieldPath>);

impl Seen {
	/// Records that `field` names `target`, unless an earlier field did.
	fn insert(
		&mut self,
		target: &str,
		field: FieldPath,
	) -> Result<(), DeclarationError> {
		if let Some(first) = self.0.get(target) {
			return Err(DeclarationError::Duplicate {
				at: field.into_string(),
				first: first.to_string(),
				target: target.to_owned(),
			});
		}
		self.0.insert(target.to_owned(), field);
		Ok(())
	}
}
