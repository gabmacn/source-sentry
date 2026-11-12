//! The repository a command works on, and the conversion of command-line
//! paths into repository paths.

use std::io;
use std::path::{Component, Path, PathBuf};

use source_sentry_core::claims::{Inventory, discover};
use source_sentry_core::config::{ConfigSet, project_of, project_roots};
use source_sentry_core::model::{DocumentId, RepoPath};
use source_sentry_core::ports::Snapshot;
use source_sentry_git::GitRepository;

use crate::cli::DocumentArg;
use crate::exit::Failure;

mod sources;

/// An open repository, with the directories that command-line paths are
/// resolved against.
pub(crate) struct Workspace {
	/// The path selecting the command project.
	pub(crate) scope: RepoPath,
	/// Tracked working-tree project markers.
	pub(crate) projects: std::collections::BTreeSet<RepoPath>,
	/// The repository.
	pub(crate) repository: GitRepository,
	/// The working tree, canonicalized.
	root: PathBuf,
	/// The working tree as git reports it, for messages: on Windows,
	/// without the `\\?\` prefix that canonicalizing adds.
	root_shown: PathBuf,
	/// The current directory, canonicalized, for comparing with `root`.
	current: PathBuf,
	/// `-C` as given, which commands printed for copying repeat.
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
		let working = repo.map_or(working.clone(), |path| working.join(path));
		let current = working.canonicalize().map_err(|error| {
			io_failure("cannot resolve the current directory", &error)
		})?;
		let start = working.clone();
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
		let snapshot = repository
			.working_tree_snapshot()
			.map_err(|error| Failure::backend(&error))?;
		let projects = project_roots(
			&snapshot.paths().map_err(|error| Failure::backend(&error))?,
		);
		let relative = current.strip_prefix(&root).unwrap_or(Path::new(""));
		let scope = RepoPath::new(&relative.to_string_lossy().replace('\\', "/"))
			.map_err(|error| Failure::usage(error.to_string()))?;
		Ok(Self {
			scope,
			projects,
			repository,
			root,
			root_shown,
			current,
			repo_argument: repo.map(Path::to_path_buf),
		})
	}

	/// The repository path of a filesystem path, relative to the current
	/// directory or absolute in the platform's native form. The path need
	/// not exist, so that a deleted document can still be named.
	///
	/// # Errors
	///
	/// Exit 64 when the path lies outside the working tree or cannot be a
	/// repository path.
	pub(crate) fn repo_path(&self, argument: &Path) -> Result<RepoPath, Failure> {
		let absolute = normalize(&self.current.join(argument));
		let relative =
			repository_relative(&absolute, &self.root).ok_or_else(|| {
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

	/// The repository path of a source spelling, relative to the current
	/// directory or to the repository root after a leading `/`.
	///
	/// # Errors
	///
	/// Exit 64 when the spelling cannot resolve within the working tree.
	pub(crate) fn source_path(
		&self,
		argument: &Path,
	) -> Result<RepoPath, Failure> {
		let written = argument.to_string_lossy();
		let absolute = if written.starts_with('/') || written.starts_with(":/") {
			let project = project_of(&self.projects, &self.scope);
			let path = source_sentry_core::model::resolve_project_source(
				&project, None, &written,
			)
			.map_err(|error| Failure::usage(error.to_string()))?;
			self.root_shown.join(path.as_str())
		} else {
			source_argument_path(&self.root_shown, &self.current, argument)?
		};
		self.repo_path(&absolute)
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
		if !self.current.starts_with(&self.root) {
			return self.root_shown.join(path.as_str()).display().to_string();
		}
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

	/// `-C` as given, if it was.
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
					"{} -C{separator}{}",
					source_sentry_core::SLUG,
					crate::commands::shell_word(&argument)
				)
			},
		)
	}
}

/// Resolves source spellings. A single leading `/` denotes the repository
/// root; all other relative paths start at the current directory.
fn source_argument_path(
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
	load_project(workspace, snapshot, &workspace.scope)
}

/// Loads a particular project for a document or placement operation.
pub(crate) fn load_project<S: Snapshot>(
	workspace: &Workspace,
	snapshot: &S,
	project: &RepoPath,
) -> Result<(ConfigSet, Inventory), Failure> {
	let configs = ConfigSet::load_project(snapshot, project)
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

/// Finds the filesystem spelling of the repository root before resolving
/// anything inside it. Everything below that boundary stays lexical, so
/// git symlinks and case-only renames retain their exact identities.
fn repository_relative(path: &Path, root: &Path) -> Option<PathBuf> {
	let ancestors: Vec<_> = path.ancestors().collect();
	for ancestor in ancestors.into_iter().rev() {
		if ancestor
			.canonicalize()
			.is_ok_and(|canonical| canonical == root)
		{
			return path.strip_prefix(ancestor).ok().map(Path::to_path_buf);
		}
	}
	None
}

/// An I/O failure while resolving directories: exit 4.
fn io_failure(context: &str, error: &io::Error) -> Failure {
	Failure::new(crate::exit::OPERATIONAL, format!("{context}: {error}"))
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Source spellings share one root resolver, including lexical parents.
	#[test]
	fn rooted_sources_use_the_repository_root() {
		let root = Path::new("repository");
		let current = root.join("docs");
		for (argument, expected) in [
			("/", "repository"),
			("/src/../a.rs", "repository/a.rs"),
			("../src/a.rs", "repository/src/a.rs"),
		] {
			assert_eq!(
				normalize(
					&source_argument_path(root, &current, Path::new(argument)).unwrap()
				),
				PathBuf::from(expected)
			);
		}
		for argument in ["//server/share", "/../outside"] {
			assert_eq!(
				source_argument_path(root, &current, Path::new(argument))
					.unwrap_err()
					.code,
				crate::exit::USAGE
			);
		}
	}
}
