//! Canonical blob oids of working-tree files.
//!
//! For each regular file, git's `filter` attribute decides the route:
//!
//! - no filter: one batched `git hash-object --stdin-paths`, without `-w`, so
//!   that `text`, `eol`, `autocrlf`, `ident` and `working-tree-encoding`
//!   apply exactly as git applies them;
//! - `filter=lfs`: [`crate::lfs`] decides what the LFS clean filter would
//!   produce, without running it. Pointers and empty files pass through the
//!   same batch with the LFS driver disabled, so git's built-in conversions
//!   still apply to them; hydrated content becomes a pointer that is hashed
//!   natively;
//! - any other filter: [`Canonical::UnsupportedFilter`], with the line that
//!   set it.
//!
//! Before the batch runs, [`crate::eol`] finds the files for which
//! `git hash-object` and `git add` would disagree, because `git add` keeps
//! CRLF line endings that the index already holds. They are reported as
//! [`Canonical::CrlfInIndex`] instead of being hashed.
//!
//! An edited placeholder, the plain file that stands for a symbolic link
//! under `core.symlinks=false`, takes the same route: `git add` keeps the
//! link's mode but converts and filters its bytes as it would a regular
//! file's. `git ls-files --eol` describes only the index blobs of regular
//! files, so it cannot show whether `git add` would keep CRLF from a link's
//! blob. Under "auto" conversion, a placeholder whose index blob may hold
//! CRLF is therefore hashed as `git add` stores it when it keeps that CRLF:
//! its line endings are left alone, and `working-tree-encoding` and `ident`
//! still apply.
//!
//! No index oid is reused: proving that a file is clean under the same
//! conversion rules as its index entry is not possible without hashing it,
//! because `assume-unchanged` and attribute changes can hide a difference.

use std::{
	collections::{BTreeMap, HashSet},
	fs,
	path::Path,
};

use source_sentry_core::ports::{
	BackendError, BackendErrorKind, Canonical, ObjectId, RepoPath,
};

use crate::{
	attributes::{
		self, Conversion, FilterLine, OriginContext, State, locate_filter,
		resolve_filter,
	},
	command::{LFS_DRIVER_DISABLED, exit_failure},
	eol::{self, AutoCrlf, Route},
	error::{self, failure},
	ids::{blob_oid, parse_printed},
	index::IndexRecord,
	lfs::{Cleaned, clean_file},
	presence::native,
	repository::GitRepository,
};

/// Text the diagnostic shows when no attribute line can be named.
const UNKNOWN_ORIGIN: &str = "origin unknown";

/// Configuration that turns off the line-ending conversion `core.autocrlf`
/// selects, and no other conversion.
const AUTOCRLF_OFF: [&str; 2] = ["-c", "core.autocrlf=false"];

/// The positions of edited placeholders whose link blob may hold CRLF text,
/// with the route of their line-ending conversion.
type CrlfPlaceholders = Vec<(usize, Route)>;

/// Computes canonical oids of regular files in one working tree.
pub(crate) struct Canonicalizer<'a> {
	repository: &'a GitRepository,
	index: &'a BTreeMap<RepoPath, IndexRecord>,
	/// The files that are edited placeholders of symbolic links.
	placeholders: HashSet<&'a RepoPath>,
}

impl<'a> Canonicalizer<'a> {
	/// Canonicalizes files of `repository`'s working tree, whose index is
	/// `index`.
	pub(crate) fn new(
		repository: &'a GitRepository,
		index: &'a BTreeMap<RepoPath, IndexRecord>,
	) -> Self {
		Self {
			repository,
			index,
			placeholders: HashSet::new(),
		}
	}

	/// Marks `placeholders`, among the files to canonicalize, as edited
	/// placeholders of symbolic links under `core.symlinks=false`.
	pub(crate) fn with_placeholders(
		mut self,
		placeholders: HashSet<&'a RepoPath>,
	) -> Self {
		self.placeholders = placeholders;
		self
	}

	/// The canonical oids of `paths`, which must be regular files in the
	/// working tree, in the order given.
	///
	/// # Errors
	///
	/// Fails when `git` fails or a file cannot be read.
	pub(crate) fn regular_files(
		&self,
		paths: &[&RepoPath],
	) -> Result<Vec<Canonical>, BackendError> {
		let work_dir = self.repository.require_work_dir()?;
		let conversions = attributes::check(self.repository.git(), paths)?;
		let mut results: Vec<Option<Canonical>> = Vec::with_capacity(paths.len());
		// Positions of the files that git itself must hash.
		let mut pending = Vec::new();
		let mut lfs_extensions = None;
		let mut reserved_drivers = None;
		for (position, (&path, conversion)) in
			paths.iter().zip(&conversions).enumerate()
		{
			let known = match conversion.filter.value() {
				None => self.reserved_driver(
					path,
					&conversion.filter,
					&mut reserved_drivers,
				)?,
				Some("lfs") => {
					self.clean_lfs(work_dir, path, conversion, &mut lfs_extensions)?
				}
				Some(driver) => Some(Canonical::UnsupportedFilter {
					filter: driver.to_owned(),
					attribute_origin: self.origin(path, driver),
				}),
			};
			if known.is_none() {
				pending.push(position);
			}
			results.push(known);
		}

		let (kept, placeholders) =
			self.kept_index_crlf(paths, &conversions, &pending)?;
		let mut placeholders = self.hash_crlf_placeholders(
			work_dir,
			paths,
			&conversions,
			&placeholders,
		)?;
		let mut batch = Vec::with_capacity(pending.len());
		let mut batch_positions = Vec::with_capacity(pending.len());
		for position in pending {
			let path = paths[position];
			if let Some(canonical) = placeholders.remove(&position) {
				results[position] = Some(canonical);
			} else if kept.contains(path) {
				results[position] = Some(Canonical::CrlfInIndex);
			} else {
				batch.push(paths[position]);
				batch_positions.push(position);
			}
		}
		for (position, id) in
			batch_positions.into_iter().zip(self.hash_paths(&batch)?)
		{
			results[position] = Some(Canonical::Oid(id));
		}
		results
			.into_iter()
			.map(|result| {
				result.ok_or_else(|| {
					failure(
						BackendErrorKind::GitExecutable,
						"`git hash-object` printed fewer IDs than it was given paths",
					)
				})
			})
			.collect()
	}

	/// The files among `pending` that `git add` would store with the CRLF
	/// line endings the index already holds, where `git hash-object` would
	/// convert them. Attributes first narrow the candidates, so
	/// that `git ls-files --eol`, which reads each file, runs only where the
	/// answer could be yes.
	///
	/// Returned second, the positions of the edited placeholders among the
	/// candidates whose index blob may hold CRLF text, with their route:
	/// `git ls-files --eol` cannot judge them, so
	/// [`Canonicalizer::hash_crlf_placeholders`] hashes them.
	fn kept_index_crlf(
		&self,
		paths: &[&RepoPath],
		conversions: &[Conversion],
		pending: &[usize],
	) -> Result<(HashSet<RepoPath>, CrlfPlaceholders), BackendError> {
		let git = self.repository.git();
		let mut autocrlf = None;
		let mut candidates = Vec::new();
		let mut placeholders = Vec::new();
		let mut encoded = HashSet::new();
		for &position in pending {
			let conversion = &conversions[position];
			let route = eol::route(conversion);
			let candidate = match route {
				Route::Auto => true,
				Route::Never => false,
				Route::Autocrlf => {
					let value = match autocrlf {
						Some(value) => value,
						None => *autocrlf.insert(eol::read_autocrlf(git)?),
					};
					value != AutoCrlf::False
				}
			};
			if !candidate || !self.index_blob_has_crlf(paths[position]) {
				continue;
			}
			if self.placeholders.contains(paths[position]) {
				placeholders.push((position, route));
				continue;
			}
			candidates.push(paths[position]);
			if conversion.working_tree_encoding.value().is_some() {
				encoded.insert(paths[position]);
			}
		}
		if candidates.is_empty() {
			return Ok((HashSet::new(), placeholders));
		}
		let autocrlf = match autocrlf {
			Some(value) => value,
			None => eol::read_autocrlf(git)?,
		};
		let kept = eol::kept_index_crlf(git, &candidates, &encoded, autocrlf)?;
		Ok((kept, placeholders))
	}

	/// The canonical oids of edited placeholders whose link blob may hold CRLF
	/// text under "auto" conversion, by position. `git add` then leaves their
	/// line endings alone but still applies `working-tree-encoding` and
	/// `ident`, and `git hash-object`, which does not read the index, would
	/// convert the line endings too. So:
	///
	/// - under `core.autocrlf`, git hashes them with `core.autocrlf=false`,
	///   which turns off only the line-ending conversion;
	/// - under a `text=auto` or `crlf=auto` attribute, a placeholder without
	///   `working-tree-encoding` or `ident` is hashed from its bytes, which git
	///   stores unchanged;
	/// - any other is hashed by `git diff`, which reads the index as
	///   `git add` does. When `git diff` prints nothing, `git add` would store
	///   the index blob, unless git skips the file because it is marked
	///   `assume-unchanged`. Then nothing can tell what `git add` would
	///   store, and the check stops with an error. (A path marked
	///   `skip-worktree` never comes here: its index blob stands for it.)
	///
	/// # Errors
	///
	/// Fails when `git` fails, a file cannot be read, or a placeholder git
	/// must hash is marked `assume-unchanged`.
	fn hash_crlf_placeholders(
		&self,
		work_dir: &Path,
		paths: &[&RepoPath],
		conversions: &[Conversion],
		placeholders: &[(usize, Route)],
	) -> Result<BTreeMap<usize, Canonical>, BackendError> {
		let auto: Vec<usize> = placeholders
			.iter()
			.filter(|(_, route)| *route == Route::Auto)
			.map(|&(position, _)| position)
			.collect();
		let auto_paths: Vec<&RepoPath> =
			auto.iter().map(|&position| paths[position]).collect();
		let idents = attributes::check_ident(self.repository.git(), &auto_paths)?;
		let mut results = BTreeMap::new();
		for (&position, ident) in auto.iter().zip(idents) {
			let path = paths[position];
			let converted = ident == State::Set
				|| conversions[position]
					.working_tree_encoding
					.value()
					.is_some();
			let canonical = if converted {
				Canonical::Oid(self.hash_converted_placeholder(path)?)
			} else {
				let bytes = fs::read(native(work_dir, path))
					.map_err(|source| error::io(format!("cannot read {path}"), source))?;
				Canonical::Oid(blob_oid(self.repository.algorithm(), &bytes)?)
			};
			results.insert(position, canonical);
		}
		let autocrlf: Vec<usize> = placeholders
			.iter()
			.filter(|(_, route)| *route == Route::Autocrlf)
			.map(|&(position, _)| position)
			.collect();
		let autocrlf_paths: Vec<&RepoPath> =
			autocrlf.iter().map(|&position| paths[position]).collect();
		let ids = self.hash_paths_with(&AUTOCRLF_OFF, &autocrlf_paths)?;
		for (position, id) in autocrlf.into_iter().zip(ids) {
			results.insert(position, Canonical::Oid(id));
		}
		Ok(results)
	}

	/// The oid `git add` would store for the edited placeholder at `path`,
	/// from `git diff` against the index. When `git diff` prints nothing,
	/// the file converts to the index blob, unless git does not compare it
	/// because it is marked `assume-unchanged`.
	///
	/// # Errors
	///
	/// Fails when `git` fails, or when the placeholder is marked
	/// `assume-unchanged`.
	fn hash_converted_placeholder(
		&self,
		path: &RepoPath,
	) -> Result<ObjectId, BackendError> {
		if let Some(id) = self.hash_against_index(path)? {
			return Ok(id);
		}
		if self.assume_unchanged(path)? {
			return Err(failure(
				BackendErrorKind::Git,
				format!(
					"`{path}` is an edited placeholder of a symbolic link that is \
					 marked assume-unchanged, so git does not compare it with the \
					 index, and what `git add` would store for it cannot be \
					 computed; clear the mark with `git update-index \
					 --no-assume-unchanged -- {path}` and check again"
				),
			));
		}
		self
			.index
			.get(path)
			.map(|record| record.oid.clone())
			.ok_or_else(|| {
				failure(BackendErrorKind::Git, format!("{path} is not in the index"))
			})
	}

	/// Whether the index marks `path` `assume-unchanged`, which
	/// `git ls-files -v` shows as a lower-case tag.
	fn assume_unchanged(&self, path: &RepoPath) -> Result<bool, BackendError> {
		let pathspec = format!(":(literal){}", path.as_str());
		let output = self
			.repository
			.git()
			.output(&["ls-files", "-v", "-z", "--", &pathspec], None)?;
		Ok(output.first().is_some_and(u8::is_ascii_lowercase))
	}

	/// The oid `git diff` gives the working-tree file at `path` against the
	/// index, which is what `git add` would store, or `None` when git prints
	/// no difference: the file converts to the index blob, or git does not
	/// compare it, as under `assume-unchanged`. The LFS driver is disabled
	/// and file system monitoring is not consulted.
	fn hash_against_index(
		&self,
		path: &RepoPath,
	) -> Result<Option<ObjectId>, BackendError> {
		let pathspec = format!(":(literal){}", path.as_str());
		let mut args = LFS_DRIVER_DISABLED.to_vec();
		args.extend([
			"-c",
			"core.fsmonitor=false",
			"diff",
			"--no-color",
			"--no-ext-diff",
			"--no-textconv",
			"--no-relative",
			"--full-index",
			"--unified=0",
			"--",
			&pathspec,
		]);
		let output = self.repository.git().output(&args, None)?;
		diff_index_line(&output)
			.map(|id| parse_printed(id, self.repository.algorithm()))
			.transpose()
	}

	/// Whether git could call `content` text with CRLF (`i/crlf` or `i/mixed`):
	/// it holds a CRLF, and none of what makes git's text detection call it
	/// binary, a NUL byte or a CR not followed by LF. Git also calls some files
	/// binary for too many unprintable bytes; those are left for
	/// `git ls-files --eol` to decide.
	fn may_be_crlf_text(content: &[u8]) -> bool {
		let mut crlf = false;
		for (index, &byte) in content.iter().enumerate() {
			match byte {
				b'\r' if content.get(index + 1) == Some(&b'\n') => crlf = true,
				0 | b'\r' => return false,
				_ => {}
			}
		}
		crlf
	}

	/// Whether `path`'s index blob may hold a CRLF. `git add` keeps CRLF from
	/// the index only when the index blob is text with CRLF (`i/crlf` or
	/// `i/mixed`), so a blob without one rules the path out before
	/// `git ls-files --eol` reads its file. That read dominated `check` under
	/// `core.autocrlf`, where nearly every file is a candidate. A blob that
	/// cannot be read keeps the path a candidate.
	fn index_blob_has_crlf(&self, path: &RepoPath) -> bool {
		let Some(oid) = self
			.index
			.get(path)
			.and_then(|record| self.repository.git_oid(&record.oid))
		else {
			return true;
		};
		self
			.repository
			.raw()
			.find_blob(oid)
			.map_or(true, |blob| Self::may_be_crlf_text(blob.content()))
	}

	/// Canonicalizes a file with `filter=lfs` as the LFS clean filter would,
	/// without running it. Returns `None` when the file passes through the
	/// filter unchanged, so that it joins the batched hash. `extensions`
	/// caches whether LFS extensions are configured.
	fn clean_lfs(
		&self,
		work_dir: &Path,
		path: &RepoPath,
		conversion: &Conversion,
		extensions: &mut Option<bool>,
	) -> Result<Option<Canonical>, BackendError> {
		let cleaned = clean_file(&native(work_dir, path))
			.map_err(|source| error::io(format!("cannot read {path}"), source))?;
		let pointer = match cleaned {
			Cleaned::PassThrough => return Ok(None),
			Cleaned::Extension => {
				return Ok(Some(Canonical::UnsupportedLfsExtension));
			}
			Cleaned::Hydrated(pointer) => pointer,
		};
		let configured = match *extensions {
			Some(configured) => configured,
			None => *extensions.insert(self.lfs_extensions_configured()?),
		};
		let canonical = if configured {
			// The clean filter would record the configured extensions in the
			// pointer.
			Canonical::UnsupportedLfsExtension
		} else if conversion.working_tree_encoding.value().is_some() {
			Canonical::Oid(self.hash_content_at(path, &pointer)?)
		} else {
			// The pointer is ASCII without a carriage return or `$`, so no
			// other built-in conversion changes it.
			Canonical::Oid(blob_oid(self.repository.algorithm(), &pointer)?)
		};
		Ok(Some(canonical))
	}

	/// Hashes working-tree files with `git hash-object --stdin-paths`, without
	/// `-w`, and with the LFS driver disabled.
	fn hash_paths(
		&self,
		paths: &[&RepoPath],
	) -> Result<Vec<ObjectId>, BackendError> {
		self.hash_paths_with(&[], paths)
	}

	/// Hashes working-tree files like [`Canonicalizer::hash_paths`], with
	/// the `-c` options `config` added.
	fn hash_paths_with(
		&self,
		config: &[&str],
		paths: &[&RepoPath],
	) -> Result<Vec<ObjectId>, BackendError> {
		if paths.is_empty() {
			return Ok(Vec::new());
		}
		let mut input = Vec::new();
		for path in paths {
			input.extend_from_slice(stdin_path(path).as_bytes());
			input.push(b'\n');
		}
		let mut args = LFS_DRIVER_DISABLED.to_vec();
		args.extend(config);
		args.extend(["hash-object", "--stdin-paths"]);
		let output = self.repository.git().output(&args, Some(&input))?;
		let ids = parse_lines(&output, self.repository)?;
		if ids.len() == paths.len() {
			Ok(ids)
		} else {
			Err(failure(
				BackendErrorKind::GitExecutable,
				format!(
					"`git hash-object` printed {} IDs for {} paths",
					ids.len(),
					paths.len()
				),
			))
		}
	}

	/// Hashes `content` as if it were stored at `path`, with the LFS driver
	/// disabled, so that only git's built-in conversions apply.
	fn hash_content_at(
		&self,
		path: &RepoPath,
		content: &[u8],
	) -> Result<ObjectId, BackendError> {
		let path_argument = format!("--path={}", path.as_str());
		let mut args = LFS_DRIVER_DISABLED.to_vec();
		args.extend(["hash-object", "--stdin", &path_argument]);
		let output = self.repository.git().output(&args, Some(content))?;
		let mut ids = parse_lines(&output, self.repository)?;
		match (ids.pop(), ids.is_empty()) {
			(Some(id), true) => Ok(id),
			_ => Err(failure(
				BackendErrorKind::GitExecutable,
				"`git hash-object --stdin` did not print exactly one ID",
			)),
		}
	}

	/// Whether git config defines LFS extensions (`lfs.extension.*`), which the
	/// clean filter would record in every new pointer.
	/// The filter of a path whose `filter` git prints as `set`, `unset` or
	/// `unspecified`. Git prints the same word for a driver literally named
	/// so (`filter=set`), and runs that driver when git config defines a
	/// `clean` or `process` command for it. Such a driver is refused like any
	/// other when an attribute line sets `filter=<word>` for the path; without
	/// that line, or without the command, git runs no filter. When the
	/// attribute files cannot show which applies, the file is refused with
	/// an unknown origin, so that a configured driver never runs. `configured`
	/// caches the configured names.
	fn reserved_driver(
		&self,
		path: &RepoPath,
		filter: &State,
		configured: &mut Option<HashSet<String>>,
	) -> Result<Option<Canonical>, BackendError> {
		let word = match filter {
			State::Set => "set",
			State::Unset => "unset",
			State::Unspecified => "unspecified",
			State::Value(_) => return Ok(None),
		};
		let configured = match configured {
			Some(names) => names,
			None => configured.insert(self.reserved_drivers_configured()?),
		};
		if !configured.contains(word) {
			return Ok(None);
		}
		let refused = |origin: String| {
			Ok(Some(Canonical::UnsupportedFilter {
				filter: word.to_owned(),
				attribute_origin: origin,
			}))
		};
		// Only a line that sets `filter=<word>` makes the word a driver's
		// name; a line that gives the state git printed, or no line where git
		// printed `unspecified`, means git runs no filter. Anything else, a
		// reading that cannot tell or that disagrees with git, may hide the
		// driver, so the file is refused.
		match self.filter_line(path) {
			FilterLine::Line { origin, state } => {
				if state.value() == Some(word) {
					refused(origin)
				} else if state == *filter {
					Ok(None)
				} else {
					refused(UNKNOWN_ORIGIN.to_owned())
				}
			}
			FilterLine::NoLine if *filter == State::Unspecified => Ok(None),
			FilterLine::NoLine | FilterLine::Inconclusive => {
				refused(UNKNOWN_ORIGIN.to_owned())
			}
		}
	}

	/// The names among `set`, `unset` and `unspecified` for which git config
	/// defines a filter driver's `clean` or `process` command.
	fn reserved_drivers_configured(
		&self,
	) -> Result<HashSet<String>, BackendError> {
		let args = [
			"config",
			"--name-only",
			"--get-regexp",
			r"^filter\.(set|unset|unspecified)\.(clean|process)$",
		];
		let finished = self.repository.git().run(&args, None)?;
		match finished.status.code() {
			Some(0) => Ok(
				String::from_utf8_lossy(&finished.stdout)
					.lines()
					.filter_map(|name| name.split('.').nth(1))
					.map(str::to_lowercase)
					.collect(),
			),
			Some(1) => Ok(HashSet::new()),
			_ => Err(exit_failure(&args, &finished)),
		}
	}

	fn lfs_extensions_configured(&self) -> Result<bool, BackendError> {
		let args = ["config", "--get-regexp", r"^lfs\.extension\."];
		let finished = self.repository.git().run(&args, None)?;
		match finished.status.code() {
			Some(0) => Ok(true),
			Some(1) => Ok(false),
			_ => Err(exit_failure(&args, &finished)),
		}
	}

	/// The attribute line that set `path`'s filter to `driver`, as
	/// `<file>:<line>`.
	fn origin(&self, path: &RepoPath, driver: &str) -> String {
		self
			.with_origin_context(|context| locate_filter(context, path, driver))
			.flatten()
			.unwrap_or_else(|| UNKNOWN_ORIGIN.to_owned())
	}

	/// The attribute line that decides `path`'s filter, with its state.
	fn filter_line(&self, path: &RepoPath) -> FilterLine {
		self
			.with_origin_context(|context| resolve_filter(context, path))
			.unwrap_or(FilterLine::Inconclusive)
	}

	/// Runs `read` over the attribute files of the working tree, or returns
	/// `None` without one, or when git cannot say where its global or system
	/// attribute file is.
	fn with_origin_context<T>(
		&self,
		read: impl FnOnce(&OriginContext<'_>) -> T,
	) -> Option<T> {
		let work_dir = self.repository.require_work_dir().ok()?;
		let global = self.repository.git_var("GIT_ATTR_GLOBAL").ok()?;
		let system = self.repository.git_var("GIT_ATTR_SYSTEM").ok()?;
		let index_blob = |file: &RepoPath| {
			let record =
				self.index.get(file).filter(|record| !record.is_gitlink())?;
			self.repository.blob_content(&record.oid).ok().flatten()
		};
		let context = OriginContext {
			work_dir,
			common_dir: self.repository.common_dir(),
			ignore_case: self.repository.config_flag("core.ignorecase"),
			global,
			system,
			index_blob: &index_blob,
		};
		Some(read(&context))
	}
}

/// A path as `git hash-object --stdin-paths` reads it. Git unquotes a line
/// that starts with `"`, so such a path is quoted. A [`RepoPath`] holds no
/// control characters or backslashes, so only `"` needs escaping.
fn stdin_path(path: &RepoPath) -> String {
	let text = path.as_str();
	if text.starts_with('"') {
		format!("\"{}\"", text.replace('"', "\\\""))
	} else {
		text.to_owned()
	}
}

/// The new blob's ID in the `index <old>..<new>[ <mode>]` line of a
/// one-file `git diff`, looked for only in the header, before the first
/// hunk or binary patch. `None` when git printed no header.
fn diff_index_line(output: &[u8]) -> Option<&str> {
	output
		.split(|&byte| byte == b'\n')
		.take_while(|line| {
			!line.starts_with(b"@@")
				&& !line.starts_with(b"Binary files ")
				&& !line.starts_with(b"GIT binary patch")
		})
		.filter_map(|line| line.strip_prefix(b"index "))
		.find_map(|rest| {
			let rest = std::str::from_utf8(rest).ok()?;
			let (_, new) = rest.split_once("..")?;
			new.split(' ').next()
		})
}

/// Parses one object ID per line.
fn parse_lines(
	output: &[u8],
	repository: &GitRepository,
) -> Result<Vec<ObjectId>, BackendError> {
	let text = std::str::from_utf8(output).map_err(|source| {
		error::caused(
			BackendErrorKind::GitExecutable,
			"`git hash-object` printed something that is not text",
			source,
		)
	})?;
	text
		.lines()
		.filter(|line| !line.is_empty())
		.map(|line| parse_printed(line.trim(), repository.algorithm()))
		.collect()
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn only_text_with_crlf_can_keep_crlf() {
		let text = Canonicalizer::may_be_crlf_text;
		assert!(text(b"a\r\nb\r\n"));
		assert!(text(b"a\r\nb\n"));
		assert!(!text(b"a\nb\n"));
		assert!(!text(b""));
		// What git's text detection calls binary.
		assert!(!text(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR"));
		assert!(!text(b"a\r\nb\0"));
		assert!(!text(b"a\r\nb\rc"));
	}

	#[test]
	fn reads_the_new_blob_from_a_diff_header_only() {
		let old = "a".repeat(40);
		let new = "b".repeat(40);
		let header = format!(
			"diff --git a/link b/link\nindex {old}..{new} 120000\n--- a/link\n\
			 +++ b/link\n@@ -1 +1 @@\n-x\n+index {old}..{old}\n"
		);
		assert_eq!(diff_index_line(header.as_bytes()), Some(new.as_str()));
		let binary = format!(
			"diff --git a/f b/f\nindex {old}..{new}\nBinary files a/f and b/f \
			 differ\n"
		);
		assert_eq!(diff_index_line(binary.as_bytes()), Some(new.as_str()));
		let body_only = format!("@@ -1 +1 @@\nindex {old}..{new}\n");
		assert_eq!(diff_index_line(body_only.as_bytes()), None);
		assert_eq!(diff_index_line(b""), None);
	}

	#[test]
	fn quotes_paths_that_start_with_a_quote() {
		let plain = RepoPath::new("docs/a \"b\".md").unwrap();
		assert_eq!(stdin_path(&plain), "docs/a \"b\".md");
		let quoted = RepoPath::new("\"odd\".md").unwrap();
		assert_eq!(stdin_path(&quoted), "\"\\\"odd\\\".md\"");
	}
}
