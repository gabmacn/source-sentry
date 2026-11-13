//! Temporary repositories and a scripted human interaction boundary.

use std::collections::VecDeque;
use std::fs;
use std::process::Command;

use serde_json::{Value, json};
use source_sentry_core::model::{DocumentId, RepoPath};
use source_sentry_core::mutate::{
	AttestRequest, DocumentDiff, Selection, attest,
};
use tempfile::TempDir;

use super::super::closing::Closing;
use super::super::opening::Opening;
use super::super::render::{self, Paint};
use super::super::{Answer, Interface, View, session};
use crate::cli::{DocumentArg, ReviewArgs};
use crate::exit::Failure;
use crate::workspace::{Workspace, load};

/// A tracked fixture repository, never the product repository.
pub(in crate::commands::review) struct Repo {
	/// Owns the directory until the session and its snapshots have closed.
	pub(super) directory: TempDir,
	/// Empty global git configuration, outside the tracked fixture files.
	global: tempfile::NamedTempFile,
}

impl Repo {
	/// Creates local documents with the supplied lockfile declarations.
	pub(in crate::commands::review) fn new(entries: Value) -> Self {
		let repo = Self {
			directory: tempfile::tempdir().unwrap(),
			global: tempfile::NamedTempFile::new().unwrap(),
		};
		repo.git(&["init", "--quiet", "--initial-branch=main"]);
		for (key, value) in [
			("user.name", "Test Reviewer"),
			("user.email", "test@example.com"),
			("commit.gpgsign", "false"),
			("core.autocrlf", "false"),
			("core.safecrlf", "false"),
		] {
			repo.git(&["config", key, value]);
		}
		repo.write(".source-sentry/source-sentry.yaml", "version: 1\n");
		repo.write("src/a.rs", "old a\n");
		repo.write("src/b.rs", "old b\n");
		for (id, _) in entries.as_object().unwrap() {
			if !id.contains(':') {
				repo.write(id, "# Document\n");
			}
		}
		let mut lockfile = json!({"schema": 1, "retired": {}});
		lockfile["documents"] = entries;
		repo.write(
			"source-sentry.lock",
			&serde_json::to_string(&lockfile).unwrap(),
		);
		repo.commit();
		repo
	}

	/// Opens the fixture without changing the process's current directory.
	pub(in crate::commands::review) fn workspace(&self) -> Workspace {
		Workspace::open(Some(self.directory.path())).unwrap()
	}

	/// Replaces a working-tree file, creating its parent directories.
	pub(in crate::commands::review) fn write(&self, path: &str, text: &str) {
		let target = self.directory.path().join(path);
		fs::create_dir_all(target.parent().unwrap()).unwrap();
		fs::write(target, text).unwrap();
	}

	/// Reads a fixture file as text.
	pub(super) fn read(&self, path: &str) -> String {
		fs::read_to_string(self.directory.path().join(path)).unwrap()
	}

	/// Reads a document's current declaration from the fixture's lockfile.
	pub(in crate::commands::review) fn entry(&self, document: &str) -> Value {
		serde_json::from_str::<Value>(&self.read("source-sentry.lock")).unwrap()["documents"][document].clone()
	}

	/// Runs git with isolated global configuration and asserts success.
	pub(in crate::commands::review) fn git(&self, args: &[&str]) -> String {
		let output = Command::new("git")
			.args(args)
			.current_dir(self.directory.path())
			.env("GIT_CONFIG_NOSYSTEM", "1")
			.env("GIT_CONFIG_GLOBAL", self.global.path())
			.env("GIT_AUTHOR_DATE", "2026-10-01T10:00:00Z")
			.env("GIT_COMMITTER_DATE", "2026-10-01T10:00:00Z")
			.output()
			.unwrap();
		assert!(
			output.status.success(),
			"git {args:?}: {}",
			String::from_utf8_lossy(&output.stderr)
		);
		String::from_utf8(output.stdout).unwrap().trim().to_owned()
	}

	/// Commits the fixture, preserving source baselines in the object database.
	pub(super) fn commit(&self) {
		self.git(&["add", "--all"]);
		self.git(&["commit", "--quiet", "--allow-empty", "-m", "Fixture"]);
	}

	/// Prepares an already reviewed document through the ordinary planner.
	pub(in crate::commands::review) fn attest(&self, document: &str) {
		let workspace = self.workspace();
		let snapshot = workspace.repository.working_tree_snapshot().unwrap();
		let (_, inventory) = load(&workspace, &snapshot).unwrap();
		let planned = attest(
			&snapshot,
			&inventory,
			&AttestRequest {
				document: local(document),
				selection: Selection::All,
				expectations: None,
				reviewer: Some("Earlier Reviewer".to_owned()),
				at: "2026-10-02T10:00:00Z".parse().unwrap(),
				reread: true,
			},
		)
		.unwrap();
		workspace.repository.apply(&planned.plan).unwrap();
	}

	/// Runs a session with an optional explicit selection and reviewer.
	pub(super) fn run(
		&self,
		selected: &[&str],
		script: &mut Script,
	) -> Result<(), Failure> {
		let args = self.args(selected);
		session::run(&self.workspace(), &args, script)
	}

	/// The arguments for a fixture session; paths are absolute because tests
	/// share the process's current directory.
	pub(in crate::commands::review) fn args(
		&self,
		selected: &[&str],
	) -> ReviewArgs {
		ReviewArgs {
			documents: selected
				.iter()
				.map(|path| DocumentArg::Path(self.directory.path().join(path)))
				.collect(),
			context: "default".to_owned(),
			reviewer: None,
			plain: false,
		}
	}
}

/// A local document identity in a fixture.
pub(super) fn local(path: &str) -> DocumentId {
	DocumentId::Local(RepoPath::new(path).unwrap())
}

/// The measure of every scripted display.
const WIDTH: usize = 100;

/// One scripted interaction, including changes made while a prompt is open.
pub(super) enum Step {
	/// A submitted answer or end of input.
	Answer(Option<Answer>),
	/// A source changes after display and before this answer is submitted.
	Change(&'static str, String, Answer),
	/// A response to the bulk confirmation, or end of input.
	Confirm(Option<bool>),
	/// An external source edit made immediately after a review is recorded.
	ChangeAfterReview(&'static str, String),
	/// The files an editor command changes before it closes.
	Edit(Vec<(&'static str, String)>),
}

/// A user who answers from a script and retains the session's evidence.
pub(super) struct Script {
	/// The fixture directory for changes performed at prompts.
	root: std::path::PathBuf,
	/// The remaining interactions.
	steps: VecDeque<Step>,
	/// Every opening presented; a session has one at most.
	pub(super) openings: Vec<Opening>,
	/// Every document shown, including retries.
	pub(super) shown: Vec<DocumentId>,
	/// Every bounded display, with paths rendered by the workspace.
	pub(super) displays: Vec<String>,
	/// Every comparison the user saw, with its exact expectations.
	pub(super) comparisons: Vec<DocumentDiff>,
	/// Session messages, including the final evaluation.
	pub(super) messages: Vec<String>,
	/// Bulk confirmation counts: remaining and unseen.
	pub(super) confirmations: Vec<(usize, usize)>,
	/// Full diffs requested by the user.
	pub(super) diffs: Vec<String>,
}

impl Script {
	/// A scripted interaction sequence; exhaustion means end of input.
	pub(super) fn new(
		repo: &Repo,
		steps: impl IntoIterator<Item = Step>,
	) -> Self {
		Self::at(repo.directory.path(), steps)
	}

	/// Scripts a session whose fixture is owned by a parent test process.
	pub(super) fn at(
		root: &std::path::Path,
		steps: impl IntoIterator<Item = Step>,
	) -> Self {
		Self {
			root: root.to_path_buf(),
			steps: steps.into_iter().collect(),
			openings: Vec::new(),
			shown: Vec::new(),
			displays: Vec::new(),
			comparisons: Vec::new(),
			messages: Vec::new(),
			confirmations: Vec::new(),
			diffs: Vec::new(),
		}
	}

	/// A session consisting only of one-letter answers.
	pub(super) fn answers(repo: &Repo, answers: &[Answer]) -> Self {
		Self::new(
			repo,
			answers.iter().map(|answer| Step::Answer(Some(*answer))),
		)
	}

	/// The final evaluation message.
	pub(super) fn summary(&self) -> &str {
		self.messages.last().unwrap()
	}
}

impl Interface for Script {
	fn open(&mut self, opening: &Opening) {
		self.openings.push(opening.clone());
	}

	fn show(&mut self, workspace: &Workspace, view: &View) {
		self.shown.push(view.compared.document.clone());
		self
			.displays
			.push(render::document(workspace, view, Paint::Plain, WIDTH));
		self.comparisons.push(view.compared.clone());
	}

	fn answer(&mut self) -> Result<Option<Answer>, Failure> {
		match self.steps.pop_front() {
			None | Some(Step::Answer(None)) => Ok(None),
			Some(Step::Answer(answer)) => Ok(answer),
			Some(Step::Change(path, content, answer)) => {
				fs::write(self.root.join(path), content).unwrap();
				let status = Command::new("git")
					.args(["add", "--", path])
					.current_dir(&self.root)
					.status()
					.unwrap();
				assert!(status.success());
				Ok(Some(answer))
			}
			_ => panic!("expected an answer"),
		}
	}

	fn confirm_all(
		&mut self,
		remaining: usize,
		unseen: usize,
	) -> Result<Option<bool>, Failure> {
		self.confirmations.push((remaining, unseen));
		match self.steps.pop_front() {
			Some(Step::Confirm(answer)) => Ok(answer),
			_ => panic!("expected a confirmation"),
		}
	}

	fn full_diff(
		&mut self,
		workspace: &Workspace,
		compared: &DocumentDiff,
	) -> Result<(), Failure> {
		self.diffs.push(render::changes(
			workspace,
			compared,
			None,
			Paint::Plain,
			WIDTH,
		));
		Ok(())
	}

	fn edit(
		&mut self,
		_workspace: &Workspace,
		_document: &DocumentId,
	) -> Result<(), Failure> {
		let Some(Step::Edit(files)) = self.steps.pop_front() else {
			panic!("expected an editor visit")
		};
		for (path, text) in files {
			fs::write(self.root.join(path), text).unwrap();
		}
		Ok(())
	}

	fn message(&mut self, text: &str) {
		self.messages.push(text.to_owned());
		if text.starts_with("Recorded ")
			&& let Some(Step::ChangeAfterReview(path, content)) = self.steps.front()
		{
			fs::write(self.root.join(path), content).unwrap();
			self.steps.pop_front();
		}
	}

	fn close(&mut self, closing: &Closing) {
		self.messages.push(closing.plain());
	}
}
