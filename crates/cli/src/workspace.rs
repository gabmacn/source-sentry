//! The repository a command works on, and the conversion of command-line
//! paths into repository paths.

use std::io;
use std::path::{Component, Path, PathBuf};

use source_sentry_core::claims::{Inventory, discover};
use source_sentry_core::config::ConfigSet;
use source_sentry_core::model::{DocumentId, RepoPath};
use source_sentry_core::ports::Snapshot;
use source_sentry_git::GitRepository;

use crate::cli::DocumentArg;
use crate::exit::Failure;

mod sources;

/// An open repository, with the directories that command-line paths are
/// resolved against.
pub(crate) struct Workspace {
	/// The repository.
	pub(crate) repository: GitRepository,
	/// The working tree, canonicalized.
	root: PathBuf,
	/// The working tree as git reports it, for messages: on Windows,
	/// without the `\\?\` prefix that canonicalizing adds.
	root_shown: PathBuf,
	/// The current directory as the process received it. Command-line paths
	/// are relative to it, as in git, even when `--repo` names another
	/// repository.
	working: PathBuf,
	/// The current directory, canonicalized, for comparing with `root`.
	current: PathBuf,
	/// `--repo` as given, which commands printed for copying repeat.
	repo_argument: Option<PathBuf>,
}

impl Workspace {
	/// Opens the repository at `repo`, or the one containing the current
	/// directory, searching upward as git does.
	///
	/// # Errors
	///
	/// Exit 4 when no repository is found, it is bare, or a directory cannot
	/// be resolved.
	pub(crate) fn open(repo: Option<&Path>) -> Result<Self, Failure> {
		// The current directory is joined and opened as given: a verbatim
		// `\\?\` path, which canonicalizing produces on Windows, keeps `..`
		// literally.
		let working = std::env::current_dir().map_err(|error| {
			io_failure("cannot read the current directory", &error)
		})?;
		let current = working.canonicalize().map_err(|error| {
			io_failure("cannot resolve the current directory", &error)
		})?;
		let start = match repo {
			Some(repo) if repo.to_string_lossy().starts_with('/') => {
				let containing = GitRepository::open(&working)
					.map_err(|error| Failure::backend(&error))?;
				let root = containing.work_dir().ok_or_else(|| {
					Failure::usage("a `/`-rooted --repo needs a working tree")
				})?;
				argument_path(root, &working, repo)?
			}
			Some(repo) => working.join(repo),
			None => working.clone(),
		};
		let repository =
			GitRepository::open(&start).map_err(|error| Failure::backend(&error))?;
		let root_shown = repository
			.work_dir()
			.ok_or_else(|| {
				Failure::new(
					crate::exit::OPERATIONAL,
					format!(
						"{} is a bare repository; Source Sentry needs a working tree",
						start.display()
					),
				)
			})?
			.to_path_buf();
		let root = root_shown
			.canonicalize()
			.map_err(|error| io_failure("cannot resolve the working tree", &error))?;
		Ok(Self {
			repository,
			root,
			root_shown,
			working,
			current,
			repo_argument: repo.map(Path::to_path_buf),
		})
	}

	/// The repository path of a command-line path, which is relative to the
	/// current directory, or to the repository root after a leading `/`.
	/// Native drive-qualified paths are also accepted. The path need not
	/// exist, so that a deleted document can still be named.
	///
	/// # Errors
	///
	/// Exit 64 when the path lies outside the working tree or cannot be a
	/// repository path.
	pub(crate) fn repo_path(&self, argument: &Path) -> Result<RepoPath, Failure> {
		let absolute =
			normalize(&argument_path(&self.root_shown, &self.working, argument)?);
		let resolved = resolve_existing(&absolute);
		let relative = resolved.strip_prefix(&self.root).map_err(|_| {
			Failure::usage(format!(
				"`{}` is outside the repository at {}",
				argument.display(),
				self.root_shown.display()
			))
		})?;
		let mut components = Vec::new();
		for component in relative.components() {
			let Component::Normal(name) = component else {
				return Err(Failure::usage(format!(
					"`{}` cannot be read as a repository path",
					argument.display()
				)));
			};
			let name = name.to_str().ok_or_else(|| {
				Failure::usage(format!("`{}` is not valid UTF-8", argument.display()))
			})?;
			components.push(name);
		}
		RepoPath::new(&components.join("/")).map_err(|error| {
			Failure::usage(format!("`{}`: {error}", argument.display()))
		})
	}

	/// The identity of a document argument.
	///
	/// # Errors
	///
	/// Exit 64 when a path cannot be converted, or names the repository root.
	pub(crate) fn document(
		&self,
		argument: &DocumentArg,
	) -> Result<DocumentId, Failure> {
		match argument {
			DocumentArg::Remote(identity) => Ok(DocumentId::Remote(identity.clone())),
			DocumentArg::Path(path) => {
				let resolved = self.repo_path(path)?;
				if resolved.is_root() {
					return Err(Failure::usage(format!(
						"`{}` is the repository root, not a document",
						path.display()
					)));
				}
				Ok(DocumentId::Local(resolved))
			}
		}
	}

	/// How to name `document` in a command printed for copying: a path
	/// relative to the current directory, with `/` separators, or the remote
	/// identity.
	pub(crate) fn display_document(&self, document: &DocumentId) -> String {
		match document {
			DocumentId::Remote(identity) => identity.to_string(),
			DocumentId::Local(path) => self.display_path(path),
		}
	}

	/// `path` relative to the current directory, with `/` separators.
	pub(crate) fn display_path(&self, path: &RepoPath) -> String {
		let target: Vec<String> = self
			.root
			.components()
			.map(|component| component.as_os_str().to_string_lossy().into_owned())
			.chain(path.components().map(str::to_owned))
			.collect();
		let current: Vec<String> = self
			.current
			.components()
			.map(|component| component.as_os_str().to_string_lossy().into_owned())
			.collect();
		let common = target
			.iter()
			.zip(&current)
			.take_while(|(left, right)| left == right)
			.count();
		if common == 0 {
			return self.root_shown.join(path.as_str()).display().to_string();
		}
		let mut parts: Vec<&str> = vec![".."; current.len() - common];
		parts.extend(target[common..].iter().map(String::as_str));
		if parts.is_empty() {
			".".to_owned()
		} else {
			parts.join("/")
		}
	}

	/// `--repo` as given, if it was.
	pub(crate) fn repo_argument(&self) -> Option<&Path> {
		self.repo_argument.as_deref()
	}

	/// The command prefix for a hint, including the repository argument.
	pub(crate) fn command_prefix(&self) -> String {
		self.repo_argument().map_or_else(
			|| source_sentry_core::SLUG.to_owned(),
			|repo| {
				let argument = repo.display().to_string();
				let separator = if argument.starts_with('-') { "=" } else { " " };
				format!(
					"{} --repo{separator}{}",
					source_sentry_core::SLUG,
					crate::commands::shell_word(&argument)
				)
			},
		)
	}
}

/// Resolves command-line paths. A single leading `/` denotes the repository
/// root; all other relative paths start at the current directory.
fn argument_path(
	root: &Path,
	current: &Path,
	argument: &Path,
) -> Result<PathBuf, Failure> {
	let written = argument.to_string_lossy();
	if let Some(relative) = written.strip_prefix('/') {
		if relative.starts_with('/') {
			return Err(Failure::usage(format!(
				"`{written}` starts with `//`; use a single `/` for the repository root"
			)));
		}
		// Normalize before joining so `..` cannot leave the repository.
		let relative = RepoPath::new(relative)
			.map_err(|error| Failure::usage(error.to_string()))?;
		Ok(root.join(relative.as_str()))
	} else {
		Ok(current.join(argument))
	}
}

/// Loads the configs of `snapshot` and discovers its inventory.
///
/// # Errors
///
/// Exit 3 for invalid configs, and 4 when the snapshot cannot be read.
pub(crate) fn load<S: Snapshot>(
	workspace: &Workspace,
	snapshot: &S,
) -> Result<(ConfigSet, Inventory), Failure> {
	let configs = ConfigSet::load(snapshot)
		.map_err(|error| Failure::load(workspace, &error))?;
	let inventory =
		discover(snapshot, &configs).map_err(|error| Failure::backend(&error))?;
	Ok((configs, inventory))
}

/// Resolves `.` and `..` lexically, as git does for pathspecs.
fn normalize(path: &Path) -> PathBuf {
	let mut normalized = PathBuf::new();
	for component in path.components() {
		match component {
			Component::CurDir => {}
			Component::ParentDir => {
				normalized.pop();
			}
			Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
				normalized.push(component.as_os_str());
			}
		}
	}
	normalized
}

/// Canonicalizes the longest existing ancestor of `path`, which is
/// absolute and normalized, and appends the rest, so that a path that does
/// not exist still compares with the canonical working tree.
fn resolve_existing(path: &Path) -> PathBuf {
	let mut missing = Vec::new();
	let mut existing = path;
	loop {
		if let Ok(canonical) = existing.canonicalize() {
			let mut resolved = canonical;
			for name in missing.iter().rev() {
				resolved.push(name);
			}
			return resolved;
		}
		match (existing.parent(), existing.file_name()) {
			(Some(parent), Some(name)) => {
				missing.push(name.to_os_string());
				existing = parent;
			}
			_ => return path.to_path_buf(),
		}
	}
}

/// An I/O failure while resolving directories: exit 4.
fn io_failure(context: &str, error: &io::Error) -> Failure {
	Failure::new(crate::exit::OPERATIONAL, format!("{context}: {error}"))
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Leading slashes share one root resolver, including lexical parents.
	#[test]
	fn rooted_arguments_use_the_repository_root() {
		let root = Path::new("repository");
		let current = root.join("docs");
		for (argument, expected) in [
			("/", "repository"),
			("/src/../a.rs", "repository/a.rs"),
			("../src/a.rs", "repository/src/a.rs"),
		] {
			assert_eq!(
				normalize(&argument_path(root, &current, Path::new(argument)).unwrap()),
				PathBuf::from(expected)
			);
		}
		for argument in ["//server/share", "/../outside"] {
			assert_eq!(
				argument_path(root, &current, Path::new(argument))
					.unwrap_err()
					.code,
				crate::exit::USAGE
			);
		}
	}
}
