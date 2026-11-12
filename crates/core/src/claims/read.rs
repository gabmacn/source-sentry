//! Reading claims and tombstones: local lockfiles, remote owner lockfiles and
//! on-file declarations.

use std::collections::{BTreeMap, BTreeSet};

use super::{
	Claim, Diagnostic, DiagnosticKind, Directories, Location, LockfileInfo,
	LockfileKind, Mode, Presence, Tombstone, in_tool_directory, is_own_file,
};
use crate::adapters::frontmatter::{self, FrontmatterError};
use crate::adapters::lockfile::{LocalLockfile, LockfileError, RemoteLockfile};
use crate::adapters::{DeclarationError, Position};
use crate::model::{DocumentId, RepoPath};
use crate::ports::{BackendError, Snapshot};
use crate::{DIR_NAME, LOCKFILE_NAME, REMOTES_DIR_NAME};

/// The field of an on-file declaration, as claims and diagnostics name it.
pub const FRONTMATTER_FIELD: &str = "source-sentry";

/// What reading produced.
#[derive(Default)]
pub(super) struct Found {
	pub(super) lockfiles: Vec<LockfileInfo>,
	pub(super) claims: Vec<Claim>,
	pub(super) tombstones: Vec<Tombstone>,
}

/// The kind of lockfile at `path`, or `None` when `path` is not one.
///
/// A local lockfile is any `source-sentry.lock` outside `.source-sentry/`.
/// A remote owner lockfile is a direct child of `.source-sentry/remotes/`
/// named `<owner>.lock` with a non-empty owner, as the config module finds
/// paired configs.
pub(super) fn lockfile_kind(path: &RepoPath) -> Option<LockfileKind> {
	if in_tool_directory(path) {
		let text = path.as_str();
		let inside = text.strip_prefix(&format!("{DIR_NAME}/")).or_else(|| {
			text
				.split_once(&format!("/{DIR_NAME}/"))
				.map(|(_, inside)| inside)
		})?;
		let file = inside.strip_prefix(REMOTES_DIR_NAME)?.strip_prefix('/')?;
		let owner = file.strip_suffix(".lock")?;
		return (!owner.is_empty() && !owner.contains('/')).then(|| {
			LockfileKind::Remote {
				owner: owner.to_owned(),
			}
		});
	}
	(path.file_name() == Some(LOCKFILE_NAME)).then(|| LockfileKind::Local {
		directory: path.parent().unwrap_or(RepoPath::ROOT),
	})
}

/// The field of a lockfile entry, written as the adapters write it, such as
/// `documents["a.md"]`.
///
/// `section` is `documents` or `retired`, and `key` is the entry's key as
/// written: a path relative to the lockfile's directory, or a remote
/// identity. [`Claim::field`] uses this spelling; [`Tombstone::field`]
/// appends the tombstone's list index.
#[must_use]
pub fn entry_field(section: &str, key: &str) -> String {
	format!("{section}[{key:?}]")
}

/// Reads every lockfile among `paths` into `found`. A lockfile that fails to
/// parse is recorded as unparsed, with a diagnostic.
///
/// # Errors
///
/// Fails when the snapshot cannot read a lockfile.
pub(super) fn lockfiles<S: Snapshot>(
	snapshot: &S,
	paths: &[RepoPath],
	tracked: &BTreeSet<RepoPath>,
	project: &RepoPath,
	found: &mut Found,
	diagnostics: &mut Vec<Diagnostic>,
) -> Result<(), BackendError> {
	for path in paths {
		let relative = path.strip_prefix(project).unwrap_or(path.as_str());
		let Some(kind) =
			lockfile_kind(&RepoPath::new(relative).map_err(|error| {
				BackendError::new(crate::ports::BackendErrorKind::Io, error.to_string())
			})?)
			.map(|kind| match kind {
				LockfileKind::Local { .. } => LockfileKind::Local {
					directory: path.parent().unwrap_or(RepoPath::ROOT),
				},
				remote @ LockfileKind::Remote { .. } => remote,
			})
		else {
			continue;
		};
		let bytes = snapshot.read(path)?;
		let parsed = match &kind {
			LockfileKind::Local { directory } => LocalLockfile::parse_in_project(
				project, directory, &bytes,
			)
			.map(|lockfile| {
				add_local(path, lockfile, &lockfile_lines(&bytes), tracked, found);
			}),
			LockfileKind::Remote { owner } => {
				RemoteLockfile::parse_in_project(project, &bytes).map(|lockfile| {
					add_remote(path, owner, lockfile, &lockfile_lines(&bytes), found);
				})
			}
		};
		if let Err(error) = &parsed {
			diagnostics.push(invalid_lockfile(path, error));
		}
		found.lockfiles.push(LockfileInfo {
			path: path.clone(),
			kind,
			parsed: parsed.is_ok(),
		});
	}
	Ok(())
}

/// Positions for a successfully parsed JSON lockfile, in diagnostic field
/// spelling. The shared YAML walker also reads JSON with key and item spans.
/// Build this map once per lockfile, before distributing it among entries.
fn lockfile_lines(bytes: &[u8]) -> BTreeMap<String, u64> {
	let Ok(text) = std::str::from_utf8(bytes) else {
		return BTreeMap::new();
	};
	crate::yaml::field_lines_with(text, |path, key| match path {
		"" => key.to_owned(),
		"documents" | "retired" => entry_field(path, key),
		_ => format!("{path}.{key}"),
	})
	.unwrap_or_default()
	.into_iter()
	// Match frontmatter's line-only locations. The JSON report deliberately
	// omits columns, even though text locations can display them.
	.map(|(field, (line, _))| (field, line))
	.collect()
}

/// Only `field` and its children. Their keys are contiguous in the sorted
/// map, so looking up an entry does not scan or copy the whole lockfile.
fn entry_lines(
	lines: &BTreeMap<String, u64>,
	field: &str,
) -> BTreeMap<String, u64> {
	let prefix = format!("{field}.");
	lines
		.range(field.to_owned()..)
		.take_while(|(key, _)| key.as_str() == field || key.starts_with(&prefix))
		.map(|(key, &line)| (key.clone(), line))
		.collect()
}

/// Adds a local lockfile's documents and tombstones.
fn add_local(
	path: &RepoPath,
	lockfile: LocalLockfile,
	lines: &BTreeMap<String, u64>,
	tracked: &BTreeSet<RepoPath>,
	found: &mut Found,
) {
	let LocalLockfile {
		directory,
		documents,
		retired,
	} = lockfile;
	// Parsing has checked that every key lies strictly inside `directory`.
	let key = |document: &RepoPath| {
		document
			.strip_prefix(&directory)
			.unwrap_or(document.as_str())
			.to_owned()
	};
	let presence = |document: &RepoPath| {
		if tracked.contains(document) {
			Presence::Present
		} else {
			Presence::Missing
		}
	};
	for (document, declaration) in documents {
		let field = entry_field("documents", &key(&document));
		let presence = presence(&document);
		found.claims.push(Claim {
			document: DocumentId::Local(document),
			mode: Mode::LocalLockfile,
			declared_in: path.clone(),
			lines: entry_lines(lines, &field),
			field,
			declaration,
			sources: Vec::new(),
			presence,
		});
	}
	for (document, tombstones) in retired {
		let presence = presence(&document);
		for (index, retired) in tombstones.into_iter().enumerate() {
			let field =
				format!("{}[{index}]", entry_field("retired", &key(&document)));
			found.tombstones.push(Tombstone {
				document: DocumentId::Local(document.clone()),
				lockfile: path.clone(),
				lines: entry_lines(lines, &field),
				field,
				retired,
				presence,
			});
		}
	}
}

/// Adds a remote owner lockfile's documents and tombstones.
fn add_remote(
	path: &RepoPath,
	owner: &str,
	lockfile: RemoteLockfile,
	lines: &BTreeMap<String, u64>,
	found: &mut Found,
) {
	for (identity, declaration) in lockfile.documents {
		let field = entry_field("documents", &identity.to_string());
		found.claims.push(Claim {
			document: DocumentId::Remote(identity),
			mode: Mode::RemoteLockfile {
				owner: owner.to_owned(),
			},
			declared_in: path.clone(),
			lines: entry_lines(lines, &field),
			field,
			declaration,
			sources: Vec::new(),
			presence: Presence::Unknown,
		});
	}
	for (identity, tombstones) in lockfile.retired {
		for (index, retired) in tombstones.into_iter().enumerate() {
			let field =
				format!("{}[{index}]", entry_field("retired", &identity.to_string()));
			found.tombstones.push(Tombstone {
				document: DocumentId::Remote(identity.clone()),
				lockfile: path.clone(),
				lines: entry_lines(lines, &field),
				field,
				retired,
				presence: Presence::Unknown,
			});
		}
	}
}

/// Reads the on-file declaration of every tracked file whose adapter can
/// carry one, eligible or not, because ignores never affect the validation of
/// existing claims. `.source-sentry/`, lockfiles and configs are
/// not documents, so they are skipped.
///
/// # Errors
///
/// Fails when the snapshot cannot read a file.
pub(super) fn on_file<S: Snapshot>(
	snapshot: &S,
	paths: &[RepoPath],
	directories: &mut Directories<'_>,
	found: &mut Found,
	diagnostics: &mut Vec<Diagnostic>,
) -> Result<(), BackendError> {
	for path in paths {
		if in_tool_directory(path) || is_own_file(path) {
			continue;
		}
		let Some(adapter) = directories
			.of(path)
			.adapter(path)
			.map(|mapping| mapping.value.adapter)
			.filter(|adapter| adapter.carries_declaration())
		else {
			continue;
		};
		let bytes = snapshot.read(path)?;
		match adapter.read_declaration(path, &bytes) {
			Ok(Some(declaration)) => found.claims.push(Claim {
				document: DocumentId::Local(path.clone()),
				mode: Mode::OnFile,
				declared_in: path.clone(),
				field: FRONTMATTER_FIELD.to_owned(),
				declaration,
				sources: Vec::new(),
				presence: Presence::Present,
				lines: frontmatter::field_lines(&bytes),
			}),
			Ok(None) => {}
			Err(error) => diagnostics.push(invalid_declaration(path, &error)),
		}
	}
	Ok(())
}

/// The diagnostic for a lockfile that failed to parse.
fn invalid_lockfile(path: &RepoPath, error: &LockfileError) -> Diagnostic {
	let (position, field) = match error {
		LockfileError::Syntax { position, .. } => (Some(*position), None),
		LockfileError::DuplicateKey { at, position, .. }
		| LockfileError::Invalid { at, position, .. } => (
			Some(*position),
			Some(at.clone()).filter(|at| !at.is_empty()),
		),
		LockfileError::UnsupportedSchema { .. } => {
			(None, Some("schema".to_owned()))
		}
		LockfileError::Key { at, .. }
		| LockfileError::InvalidTombstones { at, .. } => (None, Some(at.clone())),
		LockfileError::Declaration(error) => {
			(None, Some(declaration_field(error).to_owned()))
		}
		LockfileError::OutsideDirectory { .. } => (None, None),
	};
	Diagnostic::new(
		DiagnosticKind::InvalidLockfile,
		located(path, position, field),
		format!(
			"{error}. Until the lockfile is corrected, none of its documents or \
			 tombstones are read"
		),
	)
}

/// The diagnostic for an on-file declaration that failed to read.
fn invalid_declaration(
	path: &RepoPath,
	error: &FrontmatterError,
) -> Diagnostic {
	let line = |line: &usize| {
		Some(Position {
			line: *line,
			column: 1,
		})
	};
	let (position, field) = match error {
		FrontmatterError::DuplicateKey { second: at, .. }
		| FrontmatterError::Unlocatable { line: at }
		| FrontmatterError::CannotInsert { line: at, .. }
		| FrontmatterError::NotUtf8 { line: at }
		| FrontmatterError::NoValue { line: at }
		| FrontmatterError::BreaksYaml { line: at }
		| FrontmatterError::Ambiguous { line: Some(at) } => {
			(line(at), Some(FRONTMATTER_FIELD.to_owned()))
		}
		FrontmatterError::Ambiguous { line: None } => {
			(None, Some(FRONTMATTER_FIELD.to_owned()))
		}
		FrontmatterError::Yaml { position, at, .. } => (
			*position,
			Some(if at.is_empty() {
				FRONTMATTER_FIELD.to_owned()
			} else {
				at.clone()
			}),
		),
		FrontmatterError::Declaration(error) => {
			(None, Some(declaration_field(error).to_owned()))
		}
	};
	Diagnostic::new(
		DiagnosticKind::InvalidDeclaration,
		located(path, position, field),
		format!(
			"{error}. Until the declaration is corrected, this document is not \
			 tracked"
		),
	)
	.about(DocumentId::Local(path.clone()))
}

/// The field that a declaration error names.
fn declaration_field(error: &DeclarationError) -> &str {
	match error {
		DeclarationError::NotAllowed { at, .. }
		| DeclarationError::Unpaired { at, .. }
		| DeclarationError::Path { at, .. }
		| DeclarationError::Duplicate { at, .. }
		| DeclarationError::BlankOwner { at } => at,
	}
}

/// A location in `file`, at an adapter's position, in `field`.
fn located(
	file: &RepoPath,
	position: Option<Position>,
	field: Option<String>,
) -> Location {
	let coordinate = |value: usize| u64::try_from(value).ok();
	Location {
		file: file.clone(),
		line: position.and_then(|position| coordinate(position.line)),
		column: position.and_then(|position| coordinate(position.column)),
		field,
	}
}
