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
//! No index oid is reused: proving that a file is clean under the same
//! conversion rules as its index entry is not possible without hashing it,
//! because `assume-unchanged` and attribute changes can hide a difference.

use std::{
	collections::{BTreeMap, HashSet},
	path::Path,
};

use source_sentry_core::ports::{
	BackendError, BackendErrorKind, Canonical, ObjectId, RepoPath,
};

use crate::{
	attributes::{self, Conversion, OriginContext, State, locate_filter},
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

/// Computes canonical oids of regular files in one working tree.
pub(crate) struct Canonicalizer<'a> {
	repository: &'a GitRepository,
	index: &'a BTreeMap<RepoPath, IndexRecord>,
}

impl<'a> Canonicalizer<'a> {
	/// Canonicalizes files of `repository`'s working tree, whose index is
	/// `index`.
	pub(crate) fn new(
		repository: &'a GitRepository,
		index: &'a BTreeMap<RepoPath, IndexRecord>,
	) -> Self {
		Self { repository, index }
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

		let kept = self.kept_index_crlf(paths, &conversions, &pending)?;
		let mut batch = Vec::with_capacity(pending.len());
		let mut batch_positions = Vec::with_capacity(pending.len());
		for position in pending {
			if kept.contains(paths[position]) {
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
	fn kept_index_crlf(
		&self,
		paths: &[&RepoPath],
		conversions: &[Conversion],
		pending: &[usize],
	) -> Result<HashSet<RepoPath>, BackendError> {
		let git = self.repository.git();
		let mut autocrlf = None;
		let mut candidates = Vec::new();
		let mut encoded = HashSet::new();
		for &position in pending {
			let conversion = &conversions[position];
			let candidate = match eol::route(conversion) {
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
			if candidate && self.index_blob_has_crlf(paths[position]) {
				candidates.push(paths[position]);
				if conversion.working_tree_encoding.value().is_some() {
					encoded.insert(paths[position]);
				}
			}
		}
		if candidates.is_empty() {
			return Ok(HashSet::new());
		}
		let autocrlf = match autocrlf {
			Some(value) => value,
			None => eol::read_autocrlf(git)?,
		};
		eol::kept_index_crlf(git, &candidates, &encoded, autocrlf)
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
		if paths.is_empty() {
			return Ok(Vec::new());
		}
		let mut input = Vec::new();
		for path in paths {
			input.extend_from_slice(stdin_path(path).as_bytes());
			input.push(b'\n');
		}
		let mut args = LFS_DRIVER_DISABLED.to_vec();
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
	/// that line, or without the command, git runs no filter. `configured`
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
		// Only a line that sets `filter=<word>` makes the word a driver's
		// name; otherwise it is the state, and git runs no filter.
		let origin = self.origin(path, word);
		Ok(
			(origin != UNKNOWN_ORIGIN).then(|| Canonical::UnsupportedFilter {
				filter: word.to_owned(),
				attribute_origin: origin,
			}),
		)
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
		let Ok(work_dir) = self.repository.require_work_dir() else {
			return UNKNOWN_ORIGIN.to_owned();
		};
		let index_blob = |file: &RepoPath| {
			let record =
				self.index.get(file).filter(|record| !record.is_gitlink())?;
			self.repository.blob_content(&record.oid).ok().flatten()
		};
		let context = OriginContext {
			work_dir,
			common_dir: self.repository.common_dir(),
			ignore_case: self.repository.config_flag("core.ignorecase"),
			global: self.repository.git_var("GIT_ATTR_GLOBAL"),
			system: self.repository.git_var("GIT_ATTR_SYSTEM"),
			index_blob: &index_blob,
		};
		locate_filter(&context, path, driver)
			.unwrap_or_else(|| UNKNOWN_ORIGIN.to_owned())
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
	fn quotes_paths_that_start_with_a_quote() {
		let plain = RepoPath::new("docs/a \"b\".md").unwrap();
		assert_eq!(stdin_path(&plain), "docs/a \"b\".md");
		let quoted = RepoPath::new("\"odd\".md").unwrap();
		assert_eq!(stdin_path(&quoted), "\"\\\"odd\\\".md\"");
	}
}
