//! Lockfiles: versioned, deterministic JSON.
//!
//! A local lockfile, `<directory>/source-sentry.lock`, keys each document by
//! its canonical path relative to that directory. A remote lockfile,
//! `.source-sentry/remotes/<owner>.lock`, keys each document by its
//! [`RemoteIdentity`]. Both hold `documents`, `retired` and `schema`.
//!
//! Parsing is strict. Every object key must be unique, and objects cannot be
//! written as arrays, which `serde_json` does not check on its own (see
//! `shape`); `schema` must be [`SCHEMA`]; and every key and declaration is
//! validated for its kind of lockfile. A path may appear in both `documents`
//! and `retired`, because a new document may reuse a retired path. Every
//! retired key holds a non-empty list, oldest retirement first, with equal
//! instants kept in written order. Schema 1 uses lists even for a single
//! tombstone.
//!
//! Writing is deterministic: keys are sorted at every level, sources by
//! their declared path or pattern, and members by path; the output is
//! pretty-printed with two-space indentation, one value per line, and ends
//! with a newline. Writing a parsed canonical lockfile reproduces its bytes.

mod shape;

use std::{collections::BTreeMap, fmt};

use serde::{
	Deserialize,
	de::{Error as _, SeqAccess, Visitor},
};

use super::{
	DeclarationError, Position,
	declaration::{FieldPath, RawDeclaration, Scope, validate, validate_retired},
};
use crate::model::{
	Declaration, IdentityError, Lockfile, PathError, RemoteIdentity, RepoPath,
	Retired, SourceDecl,
};

/// The lockfile format version that this version of Source Sentry reads and
/// writes.
pub const SCHEMA: u32 = 1;

/// A parsed local lockfile, with every key resolved to a repository path.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LocalLockfile {
	/// The directory that holds the lockfile. Keys are written relative to
	/// it, so every document must lie strictly inside it.
	pub directory: RepoPath,
	/// The tracked documents, by repository path.
	pub documents: BTreeMap<RepoPath, Declaration>,
	/// Non-empty lists of tombstones by repository path, oldest retirement
	/// first; equal instants retain their written order.
	pub retired: BTreeMap<RepoPath, Vec<Retired>>,
}

/// A parsed remote owner lockfile.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RemoteLockfile {
	/// The tracked documents, by identity.
	pub documents: BTreeMap<RemoteIdentity, Declaration>,
	/// Non-empty lists of tombstones by identity, oldest retirement first;
	/// equal instants retain their written order.
	pub retired: BTreeMap<RemoteIdentity, Vec<Retired>>,
}

/// Why a lockfile could not be read or written. Every case is a configuration
/// error. Fields are named by path, such as `documents["a.md"].sources[0].oid`.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum LockfileError {
	/// The bytes are not a single JSON value.
	#[error("the lockfile is not valid JSON ({position}): {message}")]
	Syntax {
		/// Where parsing stopped.
		position: Position,
		/// What is wrong.
		message: String,
	},
	/// An object has the same key twice, as a careless merge can leave it.
	#[error("{} has the key {key:?} more than once ({position})", describe(.at))]
	DuplicateKey {
		/// The object, or empty for the top level.
		at: String,
		/// The repeated key.
		key: String,
		/// Where the repeat was found.
		position: Position,
	},
	/// `schema` is not [`SCHEMA`]. An explicit command migrates older schemas.
	#[error(
		"lockfile schema {found} is not supported; this version of Source \
		 Sentry reads schema {}",
		SCHEMA
	)]
	UnsupportedSchema {
		/// The schema found, as written.
		found: String,
	},
	/// A field is unknown, missing, or holds the wrong kind of value.
	#[error("{}: {message} ({position})", describe(.at))]
	Invalid {
		/// The field, or empty for the top level.
		at: String,
		/// Where the problem was found.
		position: Position,
		/// What is wrong.
		message: String,
	},
	/// A key of `documents` or `retired` is not a valid document key.
	#[error("`{at}`: {error}")]
	Key {
		/// The key, as a path such as `documents["./a.md"]`.
		at: String,
		/// What is wrong with it.
		error: KeyError,
	},
	/// A declaration breaks a rule of the record.
	#[error(transparent)]
	Declaration(#[from] DeclarationError),
	/// Writing: a tombstone list is empty or not in retirement order.
	#[error("`{at}`: {message}")]
	InvalidTombstones {
		/// The tombstone list.
		at: String,
		/// What is wrong with it.
		message: &'static str,
	},
	/// Writing: a document of a [`LocalLockfile`] is not strictly inside its
	/// directory.
	#[error("`{path}` is not inside the lockfile's directory `{directory}`")]
	OutsideDirectory {
		/// The document.
		path: RepoPath,
		/// The lockfile's directory.
		directory: RepoPath,
	},
}

/// `the lockfile` for the top level, or the path in backticks.
fn describe(at: &str) -> String {
	if at.is_empty() {
		"the lockfile".to_owned()
	} else {
		format!("`{at}`")
	}
}

/// Why a key of `documents` or `retired` was rejected.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum KeyError {
	/// A local key that is empty or names the lockfile's own directory.
	#[error("the key names the lockfile's own directory, not a document")]
	Directory,
	/// A local key that starts with `/`.
	#[error(
		"keys are relative to the lockfile's directory, so they cannot start \
		 with `/`"
	)]
	Rooted,
	/// A local key that leaves the lockfile's directory.
	#[error("keys cannot climb out of the lockfile's directory")]
	Climbs,
	/// A local key that is not spelled canonically.
	#[error("the key is not canonical; write it as `{canonical}`")]
	NotCanonical {
		/// The canonical spelling.
		canonical: String,
	},
	/// A local key that names a lockfile, which lockfiles never declare.
	#[error("a lockfile declares documents, never other lockfiles")]
	Lockfile,
	/// A local key that no repository path may use.
	#[error(transparent)]
	Path(PathError),
	/// A remote key that is not an identity.
	#[error(transparent)]
	Identity(IdentityError),
}

impl LocalLockfile {
	/// An empty lockfile in `directory`.
	#[must_use]
	pub fn new(directory: RepoPath) -> Self {
		Self {
			directory,
			documents: BTreeMap::new(),
			retired: BTreeMap::new(),
		}
	}

	/// Parses the lockfile in `directory` from its bytes.
	///
	/// # Errors
	///
	/// Fails when the bytes are not valid JSON or repeat a key in any object;
	/// when `schema` is not [`SCHEMA`]; when a field is unknown, missing or
	/// malformed; when a key is not a canonical path strictly inside `directory`
	/// (no leading `/`, `./`, `..` or empty component) or names a lockfile; and
	/// when a tombstone list is empty or not in chronological retirement order;
	/// when a declaration carries `url`, has a path source with only one of `oid`
	/// and `reviewed`, repeats a source, or has a source path that does not
	/// resolve from its document.
	pub fn parse(
		directory: &RepoPath,
		bytes: &[u8],
	) -> Result<Self, LockfileError> {
		let raw = parse_raw(bytes)?;
		let mut lockfile = Self::new(directory.clone());
		for (key, declaration) in raw.documents {
			let at = FieldPath::root("documents").entry(&key);
			let path =
				local_key(directory, &key).map_err(|error| LockfileError::Key {
					at: at.to_string(),
					error,
				})?;
			let declaration = Declaration::from(declaration);
			validate(&declaration, Scope::Local(&path), &at)?;
			lockfile.documents.insert(path, declaration);
		}
		for (key, retired) in raw.retired {
			let at = FieldPath::root("retired").entry(&key);
			let path =
				local_key(directory, &key).map_err(|error| LockfileError::Key {
					at: at.to_string(),
					error,
				})?;
			validate_tombstones(&retired.0, Scope::Local(&path), &at)?;
			lockfile.retired.insert(path, retired.0);
		}
		Ok(lockfile)
	}

	/// Writes the lockfile in its canonical form.
	///
	/// # Errors
	///
	/// Fails when a document is not strictly inside [`Self::directory`],
	/// when its key would name a lockfile, and when a declaration breaks a
	/// rule that [`Self::parse`] checks, so that every written lockfile
	/// parses again.
	pub fn to_bytes(&self) -> Result<Vec<u8>, LockfileError> {
		let mut documents = BTreeMap::new();
		for (path, declaration) in &self.documents {
			let (key, at) = self.key_for("documents", path)?;
			validate(declaration, Scope::Local(path), &at)?;
			documents.insert(key, canonical_order(declaration));
		}
		let mut retired = BTreeMap::new();
		for (path, tombstone) in &self.retired {
			let (key, at) = self.key_for("retired", path)?;
			validate_tombstones(tombstone, Scope::Local(path), &at)?;
			retired.insert(key, tombstone.clone());
		}
		Ok(serialize(&Lockfile {
			documents,
			retired,
			schema: SCHEMA,
		}))
	}

	/// The key under which `path` is written in `section`, and its field
	/// path.
	fn key_for(
		&self,
		section: &str,
		path: &RepoPath,
	) -> Result<(String, FieldPath), LockfileError> {
		let key = path
			.strip_prefix(&self.directory)
			.filter(|key| !key.is_empty())
			.ok_or_else(|| LockfileError::OutsideDirectory {
				path: path.clone(),
				directory: self.directory.clone(),
			})?;
		let at = FieldPath::root(section).entry(key);
		local_key(&self.directory, key).map_err(|error| LockfileError::Key {
			at: at.to_string(),
			error,
		})?;
		Ok((key.to_owned(), at))
	}
}

impl RemoteLockfile {
	/// Parses a remote owner lockfile from its bytes.
	///
	/// # Errors
	///
	/// Fails as [`LocalLockfile::parse`] does for syntax, duplicate keys,
	/// `schema` and malformed fields; when a key is not a [`RemoteIdentity`]; and
	/// when a declaration carries `seal` or `completed`, or a tombstone carries
	/// `seal` or `completed`, because a remote document can be neither a record
	/// nor part of a series; or when a source or pattern is not `/`-rooted, or
	/// breaks the other source rules of [`LocalLockfile::parse`].
	pub fn parse(bytes: &[u8]) -> Result<Self, LockfileError> {
		let raw = parse_raw(bytes)?;
		let mut lockfile = Self::default();
		for (key, declaration) in raw.documents {
			let at = FieldPath::root("documents").entry(&key);
			let identity = remote_key(&key, &at)?;
			let declaration = Declaration::from(declaration);
			validate(&declaration, Scope::Remote, &at)?;
			lockfile.documents.insert(identity, declaration);
		}
		for (key, retired) in raw.retired {
			let at = FieldPath::root("retired").entry(&key);
			let identity = remote_key(&key, &at)?;
			validate_tombstones(&retired.0, Scope::Remote, &at)?;
			lockfile.retired.insert(identity, retired.0);
		}
		Ok(lockfile)
	}

	/// Writes the lockfile in its canonical form.
	///
	/// # Errors
	///
	/// Fails when a declaration or tombstone breaks a rule that
	/// [`Self::parse`] checks, so that every written lockfile parses again.
	pub fn to_bytes(&self) -> Result<Vec<u8>, LockfileError> {
		let mut documents = BTreeMap::new();
		for (identity, declaration) in &self.documents {
			let key = identity.to_string();
			validate(
				declaration,
				Scope::Remote,
				&FieldPath::root("documents").entry(&key),
			)?;
			documents.insert(key, canonical_order(declaration));
		}
		let mut retired = BTreeMap::new();
		for (identity, tombstone) in &self.retired {
			let key = identity.to_string();
			validate_tombstones(
				tombstone,
				Scope::Remote,
				&FieldPath::root("retired").entry(&key),
			)?;
			retired.insert(key, tombstone.clone());
		}
		Ok(serialize(&Lockfile {
			documents,
			retired,
			schema: SCHEMA,
		}))
	}
}

/// A lockfile as written, before its keys and declarations are validated.
#[derive(Deserialize)]
#[serde(deny_unknown_fields, expecting = "a lockfile object")]
struct RawLockfile {
	documents: BTreeMap<String, RawDeclaration>,
	retired: BTreeMap<String, RawTombstones>,
	schema: u32,
}

/// A tombstone list whose non-emptiness and order are checked during
/// deserialization, so errors carry the JSON parser's line and column.
struct RawTombstones(Vec<Retired>);

impl<'de> Deserialize<'de> for RawTombstones {
	/// Validates within the sequence visitor so the JSON parser attaches the
	/// list's own position to invariant errors.
	fn deserialize<D: serde::Deserializer<'de>>(
		deserializer: D,
	) -> Result<Self, D::Error> {
		deserializer.deserialize_seq(TombstonesVisitor)
	}
}

/// Reads tombstone lists without changing their written order.
struct TombstonesVisitor;

impl<'de> Visitor<'de> for TombstonesVisitor {
	type Value = RawTombstones;

	/// Describes the required list when JSON holds another kind of value.
	fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		formatter.write_str("a non-empty list of tombstones in retirement order")
	}

	/// Reads and validates every tombstone before accepting the list.
	fn visit_seq<A: SeqAccess<'de>>(
		self,
		mut seq: A,
	) -> Result<Self::Value, A::Error> {
		let mut tombstones = Vec::new();
		while let Some(tombstone) = seq.next_element()? {
			tombstones.push(tombstone);
		}
		if let Some(message) = tombstone_list_error(&tombstones) {
			return Err(A::Error::custom(message));
		}
		Ok(RawTombstones(tombstones))
	}
}

/// Why `tombstones` breaks a list invariant, if it does. Equal instants
/// need no tie-breaker: their written order is preserved.
fn tombstone_list_error(tombstones: &[Retired]) -> Option<&'static str> {
	if tombstones.is_empty() {
		Some("a retired entry must hold at least one tombstone")
	} else if tombstones
		.windows(2)
		.any(|pair| pair[0].retired.at > pair[1].retired.at)
	{
		Some(
			"tombstones must be in chronological order by `retired.at`, oldest first",
		)
	} else {
		None
	}
}

/// Checks list invariants and every tombstone's fields for reading and
/// writing. Each field path includes its tombstone's index.
fn validate_tombstones(
	tombstones: &[Retired],
	scope: Scope<'_>,
	at: &FieldPath,
) -> Result<(), LockfileError> {
	if let Some(message) = tombstone_list_error(tombstones) {
		return Err(LockfileError::InvalidTombstones {
			at: at.to_string(),
			message,
		});
	}
	for (index, tombstone) in tombstones.iter().enumerate() {
		validate_retired(tombstone, scope, &at.index(index))?;
	}
	Ok(())
}

/// Parses `bytes` up to the point where keys need interpreting.
fn parse_raw(bytes: &[u8]) -> Result<RawLockfile, LockfileError> {
	shape::check(bytes)?;
	check_schema(bytes)?;
	let mut deserializer = serde_json::Deserializer::from_slice(bytes);
	// `shape::check` has already rejected anything after the value.
	let raw: RawLockfile = serde_path_to_error::deserialize(&mut deserializer)
		.map_err(|error| {
			let (position, message) = position_of(error.inner());
			LockfileError::Invalid {
				at: FieldPath::from_serde(error.path()).into_string(),
				position,
				message,
			}
		})?;
	// `check_schema` has accepted the value already; checking the typed field
	// as well keeps this function correct on its own.
	if raw.schema != SCHEMA {
		return Err(LockfileError::UnsupportedSchema {
			found: raw.schema.to_string(),
		});
	}
	Ok(raw)
}

/// Rejects any `schema` other than [`SCHEMA`] before anything else is
/// interpreted, since another schema may lay its fields out differently. A
/// missing `schema` is left to the full parse, which reports it.
fn check_schema(bytes: &[u8]) -> Result<(), LockfileError> {
	let value: serde_json::Value =
		serde_json::from_slice(bytes).map_err(|error| {
			let (position, message) = position_of(&error);
			LockfileError::Syntax { position, message }
		})?;
	match value.get("schema") {
		Some(schema) if schema.as_u64() != Some(u64::from(SCHEMA)) => {
			Err(LockfileError::UnsupportedSchema {
				found: schema.to_string(),
			})
		}
		_ => Ok(()),
	}
}

/// Resolves a local key against `directory`.
///
/// The key must be exactly the canonical spelling of a path strictly inside
/// `directory`: no leading `/`, no `.`, `..` or empty components, no
/// trailing `/`, and not a lockfile.
fn local_key(directory: &RepoPath, key: &str) -> Result<RepoPath, KeyError> {
	if key.starts_with('/') {
		return Err(KeyError::Rooted);
	}
	let relative = RepoPath::new(key).map_err(|error| match error {
		PathError::EscapesRepository(_) => KeyError::Climbs,
		error => KeyError::Path(error),
	})?;
	if relative.is_root() {
		return Err(KeyError::Directory);
	}
	if relative.as_str() != key {
		return Err(KeyError::NotCanonical {
			canonical: relative.as_str().to_owned(),
		});
	}
	if relative.file_name() == Some(crate::LOCKFILE_NAME) {
		return Err(KeyError::Lockfile);
	}
	directory.join(key).map_err(KeyError::Path)
}

/// Parses a remote key.
fn remote_key(
	key: &str,
	at: &FieldPath,
) -> Result<RemoteIdentity, LockfileError> {
	key.parse().map_err(|error| LockfileError::Key {
		at: at.to_string(),
		error: KeyError::Identity(error),
	})
}

/// `declaration` with its sources sorted by declared path or pattern, a
/// path before a pattern of the same spelling, and each pattern's members
/// sorted by path. Validation has ruled out other ties.
fn canonical_order(declaration: &Declaration) -> Declaration {
	let rank = |source: &SourceDecl| match source {
		SourceDecl::Path(_) => 0,
		SourceDecl::Pattern(_) => 1,
	};
	let mut declaration = declaration.clone();
	declaration.sources.sort_by(|a, b| {
		a.declared()
			.cmp(b.declared())
			.then_with(|| rank(a).cmp(&rank(b)))
	});
	for source in &mut declaration.sources {
		if let SourceDecl::Pattern(pattern) = source {
			pattern.members.sort_by(|a, b| a.path.cmp(&b.path));
		}
	}
	declaration
}

/// The canonical bytes of `lockfile`.
fn serialize(lockfile: &Lockfile) -> Vec<u8> {
	// `serde_json` fails only when a `Serialize` implementation fails or a
	// map key is not a string. Every map here is keyed by `String`, and the
	// model's implementations (derived structs, strings, numbers and chrono's
	// dates) never fail.
	#[expect(
		clippy::expect_used,
		reason = "string-keyed maps of the model's types always serialize"
	)]
	let mut text = serde_json::to_string_pretty(lockfile)
		.expect("a lockfile of string-keyed maps always serializes");
	text.push('\n');
	text.into_bytes()
}

/// A `serde_json` error's position, and its message without the
/// ` at line N column M` that `serde_json` appends.
fn position_of(error: &serde_json::Error) -> (Position, String) {
	let position = Position {
		line: error.line(),
		column: error.column(),
	};
	let full = error.to_string();
	let suffix = format!(" at line {} column {}", error.line(), error.column());
	let message = full.strip_suffix(&suffix).unwrap_or(&full).to_owned();
	(position, message)
}

#[cfg(test)]
mod tests;
