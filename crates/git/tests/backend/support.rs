//! Temporary repositories driven by the `git` command line.

use std::{
	fs,
	path::{Path, PathBuf},
	process::Command,
};

use source_sentry_core::ports::{CommitId, HashAlgorithm, ObjectId, RepoPath};
use source_sentry_git::GitRepository;
use tempfile::TempDir;

/// A repository in a temporary directory, removed when dropped.
pub struct TestRepo {
	/// Holds the repository and the empty global configuration.
	root: TempDir,
	/// The working tree.
	path: PathBuf,
}

impl TestRepo {
	/// A new SHA-1 repository on branch `main`, with a fixed identity and no
	/// line ending conversion unless a test asks for it.
	pub fn init() -> Self {
		Self::init_with_format("sha1")
	}

	/// A new repository like [`TestRepo::init`], whose object IDs use
	/// `format`: `sha1` or `sha256`.
	pub fn init_with_format(format: &str) -> Self {
		let root = tempfile::tempdir().expect("temporary directory");
		fs::write(root.path().join("global.gitconfig"), "").expect("global config");
		let path = root.path().join("repo");
		fs::create_dir(&path).expect("repository directory");
		let repo = Self { root, path };
		repo.git(&[
			"init",
			"--quiet",
			"--initial-branch=main",
			&format!("--object-format={format}"),
		]);
		for (key, value) in [
			("user.name", "Test Author"),
			("user.email", "author@example.com"),
			("commit.gpgsign", "false"),
			("core.autocrlf", "false"),
			("core.safecrlf", "false"),
		] {
			repo.git(&["config", key, value]);
		}
		repo
	}

	/// A clone of `origin` made through the `file://` transport, so that
	/// `--depth` applies.
	pub fn clone_from(origin: &TestRepo, depth: Option<u32>) -> Self {
		let root = tempfile::tempdir().expect("temporary directory");
		fs::write(root.path().join("global.gitconfig"), "").expect("global config");
		let path = root.path().join("repo");
		let depth_argument = depth.map(|depth| format!("--depth={depth}"));
		let mut args = vec!["clone", "--quiet"];
		if let Some(depth) = &depth_argument {
			args.push(depth);
		}
		let url = file_url(&origin.path);
		let target = path.to_string_lossy().into_owned();
		args.extend([url.as_str(), target.as_str()]);
		run_git(root.path(), root.path(), &args, &[]);
		Self { root, path }
	}

	/// The working tree.
	pub fn path(&self) -> &Path {
		&self.path
	}

	/// Runs `git` in the working tree and returns its trimmed output.
	pub fn git(&self, args: &[&str]) -> String {
		run_git(self.root.path(), &self.path, args, &[])
	}

	/// Runs `git` with extra environment variables.
	pub fn git_with_env(&self, args: &[&str], env: &[(&str, &str)]) -> String {
		run_git(self.root.path(), &self.path, args, env)
	}

	/// Writes a file, creating its directories.
	pub fn write(&self, path: &str, contents: impl AsRef<[u8]>) {
		let full = self.path.join(path);
		fs::create_dir_all(full.parent().expect("parent")).expect("directories");
		fs::write(full, contents).expect("write");
	}

	/// Reads a file of the working tree.
	pub fn read(&self, path: &str) -> Vec<u8> {
		fs::read(self.path.join(path)).expect("read")
	}

	/// Stages everything and commits it, returning the new commit.
	pub fn commit_all(&self, message: &str) -> CommitId {
		self.git(&["add", "--all"]);
		self.git(&["commit", "--quiet", "--message", message]);
		self.head()
	}

	/// Commits a real submodule at `vendor/lib`, with a gitfile and checked-out
	/// files. Both repositories use the isolated test configuration.
	pub fn add_populated_submodule(&self) -> CommitId {
		let origin = Self::init();
		origin.write("README.md", "submodule readme\n");
		origin.write("src/lib.rs", "pub fn library() {}\n");
		origin.commit_all("add library files");
		self.git(&[
			"-c",
			"protocol.file.allow=always",
			"submodule",
			"add",
			origin.path().to_str().expect("UTF-8 submodule origin"),
			"vendor/lib",
		]);
		assert!(self.path.join("vendor/lib/.git").is_file());
		assert_eq!(self.read("vendor/lib/README.md"), b"submodule readme\n");
		assert_eq!(self.read("vendor/lib/src/lib.rs"), b"pub fn library() {}\n");
		self.commit_all("add populated submodule")
	}

	/// The commit `HEAD` points to.
	pub fn head(&self) -> CommitId {
		commit_id(&self.git(&["rev-parse", "HEAD"]))
	}

	/// The blob of `path` in `revision`, as git reports it.
	pub fn blob_at(&self, revision: &str, path: &str) -> ObjectId {
		object_id(&self.git(&["rev-parse", &format!("{revision}:{path}")]))
	}

	/// The ID git gives a blob with `contents`.
	pub fn hash(&self, contents: &[u8]) -> ObjectId {
		let temporary = self.root.path().join("hash-input");
		fs::write(&temporary, contents).expect("hash input");
		let printed = run_git(
			self.root.path(),
			&self.path,
			&["hash-object", "--no-filters", &temporary.to_string_lossy()],
			&[],
		);
		object_id(&printed)
	}

	/// Opens the repository with the backend.
	pub fn open(&self) -> GitRepository {
		GitRepository::open(&self.path).expect("open repository")
	}
}

/// Runs `git` in `directory` with the configuration isolated to `root`.
fn run_git(
	root: &Path,
	directory: &Path,
	args: &[&str],
	env: &[(&str, &str)],
) -> String {
	let mut command = Command::new("git");
	command
		.args(args)
		.current_dir(directory)
		.env("GIT_CONFIG_GLOBAL", root.join("global.gitconfig"))
		.env("GIT_CONFIG_NOSYSTEM", "1")
		.env_remove("GIT_DIR")
		.env_remove("GIT_WORK_TREE")
		.env_remove("GIT_INDEX_FILE");
	for (key, value) in env {
		command.env(key, value);
	}
	let output = command.output().expect("run git");
	assert!(
		output.status.success(),
		"git {args:?} failed: {}",
		String::from_utf8_lossy(&output.stderr)
	);
	String::from_utf8(output.stdout)
		.expect("UTF-8 output")
		.trim()
		.to_owned()
}

/// A `file://` URL for a local directory.
fn file_url(path: &Path) -> String {
	let text = path.to_string_lossy().replace('\\', "/");
	if text.starts_with('/') {
		format!("file://{text}")
	} else {
		format!("file:///{text}")
	}
}

/// A repository path.
pub fn path(text: &str) -> RepoPath {
	RepoPath::new(text).expect("valid path")
}

/// An object ID from its digits, SHA-1 or SHA-256 by their number.
pub fn object_id(hex: &str) -> ObjectId {
	let algorithm = if hex.len() == HashAlgorithm::Sha256.hex_len() {
		HashAlgorithm::Sha256
	} else {
		HashAlgorithm::Sha1
	};
	ObjectId::new(algorithm, hex).expect("object ID")
}

/// A commit ID from its digits.
pub fn commit_id(hex: &str) -> CommitId {
	CommitId(object_id(hex))
}
