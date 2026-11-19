//! The repository a command works on, and the conversion of command-line
//! paths into repository paths.

use std::io;
use std::path::{Component, Path, PathBuf};

use source_sentry_core::claims::{Inventory, discover};
use source_sentry_core::config::{ConfigSet, project_of, project_roots};
use source_sentry_core::evaluate::context_errors;
use source_sentry_core::model::{DocumentId, RepoPath};
use source_sentry_core::ports::Snapshot;
use source_sentry_git::GitRepository;

use crate::cli::DocumentArg;
use crate::exit::{self, Failure};

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
	/// The effective directory, below `root`: where git places the start
	/// directory in the working tree, or the root itself when the start
	/// lies outside it, which `GIT_WORK_TREE` allows. Relative command-line
	/// paths start there, and displayed paths are relative to it, as git
	/// takes them.
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
	/// Exit 4 when no repository is found, it is bare, a directory cannot be
	/// resolved, or `repo` is not a directory.
	pub(crate) fn open(repo: Option<&Path>) -> Result<Self, Failure> {
		// The current directory is joined and opened as given: a verbatim
		// `\\?\` path, which canonicalizing produces on Windows, keeps `..`
		// literally.
		let working = std::env::current_dir().map_err(|error| {
			io_failure("cannot read the current directory", &error)
		})?;
		let working = repo.map_or(working.clone(), |path| working.join(path));
		let resolved = working.canonicalize().map_err(|error| {
			io_failure("cannot resolve the current directory", &error)
		})?;
		// `-C` names the directory to start in, as in git, which refuses
		// any other kind of file.
		if let Some(path) = repo
			&& !resolved.is_dir()
		{
			return Err(Failure::new(
				crate::exit::OPERATIONAL,
				format!(
					"cannot change to `{}`: it is not a directory",
					path.display()
				),
			));
		}
		let start = working.clone();
		// Git finds the repository, from the start directory and git's
		// environment, so that the commands printed for the user's shell
		// reach the same one.
		let repository = GitRepository::open_from_env(&start)
			.map_err(|error| Failure::backend(&error))?;
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
		// The project is selected from where git places the start directory,
		// and from the root when it lies outside the working tree, where git
		// also takes relative paths from the root.
		let scope = repository.prefix().cloned().unwrap_or_default();
		let mut current = root.clone();
		current.extend(scope.components());
		// The backend's own advice names git commands too; they are written
		// as this workspace writes them, to run where the user started.
		let repo_argument = repo.map(Path::to_path_buf);
		let mut repository = repository;
		{
			let (root, root_shown, current, repo_argument) = (
				root.clone(),
				root_shown.clone(),
				current.clone(),
				repo_argument.clone(),
			);
			repository.set_command_style(move |arguments, paths| {
				git_command_line(
					repo_argument.as_deref(),
					arguments,
					paths
						.iter()
						.map(|path| relative_path(&root, &root_shown, &current, path)),
				)
			});
		}
		Ok(Self {
			scope,
			projects,
			repository,
			root,
			root_shown,
			current,
			repo_argument,
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
		self.repo_path_from(&self.current, argument)
	}

	/// [`Workspace::repo_path`], for a command line that starts in `start`.
	fn repo_path_from(
		&self,
		start: &Path,
		argument: &Path,
	) -> Result<RepoPath, Failure> {
		let absolute = normalize(&start.join(argument));
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
		self.source_path_from(&self.current, argument)
	}

	/// [`Workspace::source_path`], for a command line that starts in
	/// `start`.
	pub(crate) fn source_path_from(
		&self,
		start: &Path,
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
			source_argument_path(&self.root_shown, start, argument)?
		};
		self.repo_path_from(start, &absolute)
	}

	/// The directory that command lines typed for this invocation start in:
	/// the effective directory.
	pub(crate) fn start(&self) -> &Path {
		&self.current
	}

	/// The directory of `project` in the working tree, where a command line
	/// written to run from that project's root starts.
	pub(crate) fn project_directory(&self, project: &RepoPath) -> PathBuf {
		let mut directory = self.root.clone();
		directory.extend(project.components());
		directory
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

	/// `path` relative to the effective directory, with `/` separators.
	pub(crate) fn display_path(&self, path: &RepoPath) -> String {
		relative_path(&self.root, &self.root_shown, &self.current, path)
	}

	/// Where `path` is in the filesystem, for reading a working-tree file.
	pub(crate) fn file(&self, path: &RepoPath) -> PathBuf {
		let mut file = self.root_shown.clone();
		file.extend(path.components());
		file
	}

	/// `-C` as given, if it was.
	pub(crate) fn repo_argument(&self) -> Option<&Path> {
		self.repo_argument.as_deref()
	}

	/// A git command for a hint, to be pasted into the shell the user
	/// started in: `git`, then `-C` as given when it was, then `arguments`,
	/// then `--` and `paths`. The paths are displayed from the effective
	/// directory, which is where `-C` makes git resolve them, and `--` keeps
	/// a path that begins with `-` from reading as an option.
	pub(crate) fn git_command(
		&self,
		arguments: &[&str],
		paths: &[&RepoPath],
	) -> String {
		git_command_line(
			self.repo_argument(),
			arguments,
			paths.iter().map(|path| self.display_path(path)),
		)
	}

	/// A local document as a Source Sentry command argument, displayed from
	/// the effective directory, with `./` in front when it would otherwise
	/// read as an option or as a remote identity. Not yet quoted.
	pub(crate) fn document_argument(&self, path: &RepoPath) -> String {
		let shown = self.display_path(path);
		if shown.starts_with('-')
			|| matches!(
				crate::cli::parse_document(&shown),
				Ok(DocumentArg::Remote(_))
			) {
			format!("./{shown}")
		} else {
			shown
		}
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

/// `path` relative to `current`, a directory below the working tree `root`,
/// with `/` separators; `root_shown` spells the root for an absolute path.
fn relative_path(
	root: &Path,
	root_shown: &Path,
	current: &Path,
	path: &RepoPath,
) -> String {
	let target: Vec<String> = root
		.components()
		.map(|component| component.as_os_str().to_string_lossy().into_owned())
		.chain(path.components().map(str::to_owned))
		.collect();
	let current: Vec<String> = current
		.components()
		.map(|component| component.as_os_str().to_string_lossy().into_owned())
		.collect();
	let common = target
		.iter()
		.zip(&current)
		.take_while(|(left, right)| left == right)
		.count();
	if common == 0 {
		return root_shown.join(path.as_str()).display().to_string();
	}
	let mut parts: Vec<&str> = vec![".."; current.len() - common];
	parts.extend(target[common..].iter().map(String::as_str));
	if parts.is_empty() {
		".".to_owned()
	} else {
		parts.join("/")
	}
}

/// A git command for a hint: `git`, then `-C` and `repo_argument` when
/// given, then `arguments`, then `--` and the displayed `paths`, each word
/// quoted for the shell.
fn git_command_line(
	repo_argument: Option<&Path>,
	arguments: &[&str],
	paths: impl Iterator<Item = String>,
) -> String {
	let mut words = vec!["git".to_owned()];
	if let Some(repo) = repo_argument {
		words.push("-C".to_owned());
		words.push(repo.display().to_string());
	}
	words.extend(arguments.iter().map(|argument| (*argument).to_owned()));
	words.push("--".to_owned());
	words.extend(paths);
	words
		.iter()
		.map(|word| crate::commands::shell_word(word))
		.collect::<Vec<_>>()
		.join(" ")
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
/// Exit 3 for invalid configs, including an invalid context, and 4 when the
/// snapshot cannot be read.
pub(crate) fn load<S: Snapshot>(
	workspace: &Workspace,
	snapshot: &S,
) -> Result<(ConfigSet, Inventory), Failure> {
	load_project(workspace, snapshot, &workspace.scope)
}

/// Loads a particular project for a document or placement operation, and
/// refuses an invalid context of it as [`valid_contexts`] does.
///
/// # Errors
///
/// Exit 3 for invalid configs, including an invalid context, and 4 when the
/// snapshot cannot be read.
pub(crate) fn load_project<S: Snapshot>(
	workspace: &Workspace,
	snapshot: &S,
	project: &RepoPath,
) -> Result<(ConfigSet, Inventory), Failure> {
	let configs = ConfigSet::load_project(snapshot, project)
		.map_err(|error| Failure::load(workspace, &error))?;
	valid_contexts(workspace, &configs)?;
	let inventory =
		discover(snapshot, &configs).map_err(|error| Failure::backend(&error))?;
	Ok((configs, inventory))
}

/// Refuses `configs` when a context of its project is invalid: the command
/// stops, as for any other configuration error, with the diagnostics that
/// `check` prints for it. A command works in this project, so its contexts
/// are its own, unlike those of a project that a document only cites,
/// which are never validated for the citing project.
///
/// # Errors
///
/// Exit 3, listing every invalid context.
pub(crate) fn valid_contexts(
	workspace: &Workspace,
	configs: &ConfigSet,
) -> Result<(), Failure> {
	let invalid = context_errors(configs);
	if invalid.is_empty() {
		return Ok(());
	}
	let mut message = String::from("the configs are invalid:");
	for line in crate::commands::diagnostic_lines(workspace, &invalid) {
		message.push_str("\n  - ");
		message.push_str(&line);
	}
	Err(Failure::new(exit::CONFIGURATION, message))
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
