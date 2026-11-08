//! Temporary repositories driven by the `git` command line, and runs of the
//! built `source-sentry` binary inside them.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;
use tempfile::TempDir;

/// The binary under test.
const BINARY: &str = env!("CARGO_BIN_EXE_source-sentry");

/// The evaluation instant most tests check at: after every commit they
/// make.
pub const AT: &str = "2026-11-07T18:00:00Z";

/// A repository in a temporary directory, removed when dropped.
pub struct TestRepo {
	/// Holds the repository and an empty global git configuration.
	root: TempDir,
	/// The working tree.
	path: PathBuf,
}

/// What one run of the binary printed, and its exit code.
#[derive(Debug)]
pub struct Run {
	/// The exit code.
	pub code: i32,
	/// Standard output.
	pub stdout: String,
	/// Standard error.
	pub stderr: String,
}

impl Run {
	/// Standard output, parsed as JSON.
	pub fn json(&self) -> Value {
		serde_json::from_str(&self.stdout).unwrap_or_else(|error| {
			panic!("stdout is not JSON ({error}):\n{}", self.stdout)
		})
	}

	/// Asserts the exit code, showing the output when it differs.
	#[track_caller]
	pub fn expect_code(&self, code: i32) -> &Self {
		assert_eq!(
			self.code, code,
			"expected exit {code}\n--- stdout\n{}\n--- stderr\n{}",
			self.stdout, self.stderr
		);
		self
	}
}

impl TestRepo {
	/// A new repository on branch `main`, with a fixed identity, no line
	/// ending conversion, and `docs/guide.md` and `src/a.rs`, uncommitted.
	pub fn init() -> Self {
		let root = tempfile::tempdir().expect("temporary directory");
		fs::write(root.path().join("global.gitconfig"), "").expect("global config");
		let path = root.path().join("repo");
		fs::create_dir(&path).expect("repository directory");
		let repo = Self { root, path };
		repo.git(&["init", "--quiet", "--initial-branch=main"]);
		for (key, value) in [
			("user.name", "Test Author"),
			("user.email", "author@example.com"),
			("commit.gpgsign", "false"),
			("core.autocrlf", "false"),
			("core.safecrlf", "false"),
		] {
			repo.git(&["config", key, value]);
		}
		repo.write("docs/guide.md", "# Guide\n");
		repo.write("src/a.rs", "fn a() {}\n");
		repo
	}

	/// The working tree.
	pub fn path(&self) -> &Path {
		&self.path
	}

	/// Writes a file, creating its directories, and stages it, as a
	/// developer adds a new file to git: Source Sentry reads only the files
	/// that git tracks.
	pub fn write(&self, path: &str, contents: &str) {
		let full = self.path.join(path);
		fs::create_dir_all(full.parent().expect("parent")).expect("directories");
		fs::write(full, contents).expect("write");
		self.git(&["add", path]);
	}

	/// Reads a file of the working tree.
	pub fn read(&self, path: &str) -> String {
		fs::read_to_string(self.path.join(path)).expect("read")
	}

	/// Whether a file exists in the working tree.
	pub fn exists(&self, path: &str) -> bool {
		self.path.join(path).exists()
	}

	/// Runs `git` in the working tree and returns its trimmed output.
	pub fn git(&self, args: &[&str]) -> String {
		self.git_with_env(args, &[])
	}

	/// Stages everything and commits it, dated `date`.
	pub fn commit_at(&self, date: &str) {
		self.git(&["add", "--all"]);
		self.git_with_env(
			&["commit", "--quiet", "--allow-empty", "--message", date],
			&[("GIT_AUTHOR_DATE", date), ("GIT_COMMITTER_DATE", date)],
		);
	}

	/// Runs `source-sentry` with `args` in the working tree.
	pub fn run(&self, args: &[&str]) -> Run {
		self.run_in(".", args)
	}

	/// Runs `source-sentry` with `args` in `directory` of the working tree.
	pub fn run_in(&self, directory: &str, args: &[&str]) -> Run {
		let mut command = Command::new(BINARY);
		self.isolate(&mut command);
		let output = command
			.args(args)
			.current_dir(self.path.join(directory))
			.env("NO_COLOR", "1")
			.stdin(Stdio::null())
			.output()
			.expect("run source-sentry");
		Run {
			code: output.status.code().expect("an exit code"),
			stdout: String::from_utf8(output.stdout).expect("UTF-8 stdout"),
			stderr: String::from_utf8(output.stderr).expect("UTF-8 stderr"),
		}
	}

	/// Registers `docs/guide.md` against `src/a.rs`, attests it, and
	/// commits both steps in October 2026.
	pub fn reviewed_guide(&self) {
		self
			.run(&["track", "docs/guide.md", "--source", "/src/a.rs", "--yes"])
			.expect_code(0);
		self.commit_at("2026-10-01T10:00:00Z");
		self
			.run(&["attest", "docs/guide.md", "--yes"])
			.expect_code(0);
		self.commit_at("2026-10-02T10:00:00Z");
	}

	fn git_with_env(&self, args: &[&str], env: &[(&str, &str)]) -> String {
		let mut command = Command::new("git");
		self.isolate(&mut command);
		command.args(args).current_dir(&self.path);
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

	/// Keeps git, and the binary's own calls to it, away from the machine's
	/// configuration.
	fn isolate(&self, command: &mut Command) {
		command
			.env(
				"GIT_CONFIG_GLOBAL",
				self.root.path().join("global.gitconfig"),
			)
			.env("GIT_CONFIG_NOSYSTEM", "1")
			.env_remove("GIT_DIR")
			.env_remove("GIT_WORK_TREE")
			.env_remove("GIT_INDEX_FILE");
	}
}

/// The keys of a JSON object, sorted.
pub fn keys(value: &Value) -> Vec<&str> {
	let mut keys: Vec<&str> = value
		.as_object()
		.unwrap_or_else(|| panic!("not an object: {value}"))
		.keys()
		.map(String::as_str)
		.collect();
	keys.sort_unstable();
	keys
}

/// The subject of a JSON report for the local document `document`.
pub fn document<'a>(report: &'a Value, document: &str) -> &'a Value {
	report["subjects"]
		.as_array()
		.expect("subjects")
		.iter()
		.find(|subject| subject["document"] == document)
		.unwrap_or_else(|| panic!("no subject for {document} in {report:#}"))
}
