//! The clocks of remote documents: a move between owners keeps them, and the
//! walk reads no history older than they need.

use jiff::Timestamp;
use serde_json::{Value, json};

use super::{
	REPOSITORY, Repo, attested, day, evaluate, lockfile, review, started,
};
use crate::claims::discover;
use crate::config::ConfigSet;
use crate::evaluate::run::{DocumentState, Evaluation, Timing, check};
use crate::model::DocumentId;
use crate::ports::{
	BackendError, BackendErrorKind, Commit, CommitId, History, ObjectId, RepoPath,
};
use crate::testing::{FakeHistory, fake_oid, path};

/// The owner lockfile of `owner`.
fn owner(owner: &str) -> String {
	format!(".source-sentry/remotes/{owner}.lock")
}

/// An owner lockfile that declares `abstract:video` as `declaration`.
fn video(declaration: &Value) -> String {
	lockfile(&json!({"abstract:video": declaration}))
}

/// The never-reviewed declaration of `abstract:video`.
fn unattested() -> Value {
	json!({"sources": [{"path": "/src/a.rs"}]})
}

/// An owner lockfile that declares only another document.
fn unrelated() -> String {
	lockfile(&json!({"abstract:other": unattested()}))
}

/// The live evidence of `abstract:video`.
fn live(evaluation: &Evaluation) -> &crate::evaluate::run::LiveDocument {
	let id = DocumentId::Remote("abstract:video".parse().unwrap());
	let subject = evaluation
		.subjects_of(&id)
		.next()
		.unwrap_or_else(|| panic!("no subject for {id}: {evaluation:#?}"));
	let Some(DocumentState::Live(live)) =
		subject.document().map(|document| &document.state)
	else {
		panic!("abstract:video is not a live document: {subject:#?}");
	};
	live
}

/// Where the declaration clock of the never-reviewed `abstract:video`
/// started.
fn declaration_clock(evaluation: &Evaluation) -> (CommitId, Timestamp) {
	let timing = live(evaluation)
		.unattested
		.as_ref()
		.expect("an unattested declaration");
	let (commit, timestamp) = started(timing);
	(commit.clone(), timestamp)
}

/// Where the clock of the source `source` of `abstract:video` started.
fn source_clock(
	evaluation: &Evaluation,
	source: &str,
) -> (CommitId, Timestamp) {
	let report = live(evaluation)
		.sources
		.iter()
		.find(|report| report.path == path(source))
		.unwrap_or_else(|| panic!("no source {source}"));
	let (commit, timestamp) = started(&report.timing);
	(commit.clone(), timestamp)
}

/// A repository whose `abstract:video` was declared on day 1 in `legal`,
/// after a day of unrelated history.
fn declared_on_day_one() -> (Repo, CommitId) {
	let mut repo = Repo::new(&[(REPOSITORY, "version: 1\n"), ("src/a.rs", "v1")]);
	let declared =
		repo.commit(1, &[(&owner("legal"), Some(&video(&unattested())))]);
	(repo, declared)
}

/// A history whose object database lacks some commits, or the trees of
/// some commits, as a damaged repository or a treeless partial clone does.
struct Damaged {
	inner: FakeHistory,
	/// Commits whose own object is missing.
	commits: Vec<CommitId>,
	/// Commits whose tree is missing.
	trees: Vec<CommitId>,
}

impl Damaged {
	fn missing(what: &str, commit: &CommitId) -> BackendError {
		BackendError::new(
			BackendErrorKind::MissingObject,
			format!("the {what} of {commit} is missing"),
		)
	}

	fn tree(&self, commit: &CommitId) -> Result<(), BackendError> {
		if self.trees.contains(commit) || self.commits.contains(commit) {
			return Err(Self::missing("tree", commit));
		}
		Ok(())
	}
}

impl History for Damaged {
	fn first_parent_walk(
		&self,
		start: &CommitId,
	) -> impl Iterator<Item = Result<Commit, BackendError>> + '_ {
		self
			.inner
			.first_parent_walk(start)
			.map(|commit| match commit {
				Ok(commit) if self.commits.contains(&commit.id) => {
					Err(Self::missing("object", &commit.id))
				}
				other => other,
			})
	}

	fn blobs_at(
		&self,
		commit: &CommitId,
		paths: &[RepoPath],
	) -> Result<Vec<Option<ObjectId>>, BackendError> {
		self.tree(commit)?;
		self.inner.blobs_at(commit, paths)
	}

	fn blobs_in(
		&self,
		commit: &CommitId,
		directory: &RepoPath,
	) -> Result<Vec<(RepoPath, ObjectId)>, BackendError> {
		self.tree(commit)?;
		self.inner.blobs_in(commit, directory)
	}

	fn blob(&self, id: &ObjectId) -> Result<Option<Vec<u8>>, BackendError> {
		self.inner.blob(id)
	}
}

/// Evaluates a clean checkout of `repo` at `at` through `history`.
fn evaluate_through<H: History>(
	repo: &Repo,
	history: &H,
	at: Timestamp,
) -> Result<Evaluation, BackendError> {
	let snapshot = repo.checkout();
	let configs = ConfigSet::load(&snapshot).unwrap();
	let inventory = discover(&snapshot, &configs).unwrap();
	check(&snapshot, history, &configs, &inventory, at, None)
}

/// The clock of a remote declaration is settled by the commit before the
/// one that introduced it. A commit or tree older than that one is never
/// read, so its absence from the object database changes nothing; the
/// absence of that commit's own tree still fails the evaluation.
#[test]
fn a_remote_clock_never_reads_history_older_than_the_commit_that_settles_it() {
	let mut repo = Repo::new(&[(REPOSITORY, "version: 1\n"), ("src/a.rs", "v1")]);
	let root = repo.history.head().unwrap();
	let filler = repo.commit(1, &[("notes.txt", Some("one"))]);
	let before = repo.commit(2, &[("notes.txt", Some("two"))]);
	let declared =
		repo.commit(3, &[(&owner("legal"), Some(&video(&unattested())))]);
	repo.commit(4, &[("notes.txt", Some("three"))]);
	let complete = evaluate_through(&repo, &repo.history, day(5)).unwrap();
	assert_eq!(declaration_clock(&complete), (declared.clone(), day(3)));

	for damaged in [
		Damaged {
			inner: repo.history.clone(),
			commits: Vec::new(),
			trees: vec![root.clone(), filler.clone()],
		},
		Damaged {
			inner: repo.history.clone(),
			commits: vec![root.clone()],
			trees: vec![filler.clone()],
		},
	] {
		let evaluation = evaluate_through(&repo, &damaged, day(5)).unwrap();
		assert_eq!(evaluation, complete);
	}

	let needed = Damaged {
		inner: repo.history.clone(),
		commits: Vec::new(),
		trees: vec![before],
	};
	let error = evaluate_through(&repo, &needed, day(5)).unwrap_err();
	assert_eq!(error.kind, BackendErrorKind::MissingObject);
}

/// An owner lockfile the document never left is read alone: another
/// owner's lockfile is not read at a commit that the current one settles,
/// so its blob may be missing from the object database.
#[test]
fn another_owners_lockfile_is_not_read_while_the_current_one_settles_the_clock()
{
	let (mut repo, declared) = declared_on_day_one();
	let other = lockfile(
		&json!({"abstract:other": {"sources": [attested("/src/a.rs", "v1")]}}),
	);
	repo.commit(2, &[(&owner("marketing"), Some(&other))]);
	let complete = repo.evaluate(day(3), None);
	assert_eq!(declaration_clock(&complete), (declared.clone(), day(1)));

	let mut blobless = repo.history.clone();
	blobless.forget_blob(&fake_oid(other.as_bytes()));
	let snapshot = repo
		.checkout()
		.with_file(&owner("marketing"), other.as_bytes());
	let evaluation = evaluate(&blobless, &snapshot, day(3), None);
	assert_eq!(declaration_clock(&evaluation), (declared, day(1)));
}

/// Moving a remote document to another owner, then into a third owner's
/// existing lockfile, keeps its clock: the deleted lockfiles of the earlier
/// owners are still read in the commits that hold them. An uncommitted move
/// keeps it as well.
#[test]
fn moves_between_owners_keep_a_remote_clock() {
	let (mut repo, declared) = declared_on_day_one();
	repo.commit(2, &[(&owner("third"), Some(&unrelated()))]);
	repo.commit(
		3,
		&[
			(&owner("legal"), None),
			(&owner("marketing"), Some(&video(&unattested()))),
		],
	);
	let uncommitted =
		repo.checkout().without_file(&owner("marketing")).with_file(
			&owner("third"),
			lockfile(&json!({
				"abstract:other": unattested(),
				"abstract:video": unattested(),
			}))
			.as_bytes(),
		);
	let evaluation = evaluate(&repo.history, &uncommitted, day(5), None);
	assert_eq!(declaration_clock(&evaluation), (declared.clone(), day(1)));

	repo.commit(
		4,
		&[
			(&owner("marketing"), None),
			(
				&owner("third"),
				Some(&lockfile(&json!({
					"abstract:other": unattested(),
					"abstract:video": unattested(),
				}))),
			),
		],
	);
	let evaluation = repo.evaluate(day(5), None);
	assert_eq!(declaration_clock(&evaluation), (declared, day(1)));
}

/// An owner lockfile that a move deletes and a later move creates again is
/// read in every commit that holds it, so moving a document away and back
/// keeps its clock, and a declaration absent in between still ends it.
#[test]
fn an_owner_lockfile_that_disappears_and_reappears_keeps_a_remote_clock() {
	let (mut repo, declared) = declared_on_day_one();
	repo.commit(
		2,
		&[
			(&owner("legal"), None),
			(&owner("marketing"), Some(&video(&unattested()))),
		],
	);
	repo.commit(
		3,
		&[
			(&owner("marketing"), None),
			(&owner("legal"), Some(&video(&unattested()))),
		],
	);
	let evaluation = repo.evaluate(day(4), None);
	assert_eq!(declaration_clock(&evaluation), (declared, day(1)));

	// Untracked on day 4 and tracked again on day 5: the clock restarts.
	repo.commit(4, &[(&owner("legal"), None)]);
	let again = repo.commit(5, &[(&owner("legal"), Some(&video(&unattested())))]);
	let evaluation = repo.evaluate(day(6), None);
	assert_eq!(declaration_clock(&evaluation), (again, day(5)));
}

/// A reviewed remote document's new source and new pattern match keep the
/// clocks their declaration started when the document moves to another
/// owner.
#[test]
fn a_move_keeps_the_clocks_of_new_sources_and_matches() {
	let mut repo = Repo::new(&[
		(REPOSITORY, "version: 1\n"),
		("src/a.rs", "v1"),
		("src/m/one.rs", "one"),
		(
			&owner("legal"),
			&video(&json!({"sources": [attested("/src/a.rs", "v1")]})),
		),
	]);
	let reviewed = json!({"sources": [
		attested("/src/a.rs", "v1"),
		{"path": "/src/b.rs"},
		{"pattern": "/src/m/*.rs", "reviewed": review(), "members": [
			attested("/src/m/one.rs", "one"),
		]},
	]});
	let added = repo.commit(
		1,
		&[
			("src/b.rs", Some("b")),
			(&owner("legal"), Some(&video(&reviewed))),
		],
	);
	let matched = repo.commit(2, &[("src/m/two.rs", Some("two"))]);
	repo.commit(
		3,
		&[
			(&owner("legal"), None),
			(&owner("marketing"), Some(&video(&reviewed))),
		],
	);

	let evaluation = repo.evaluate(day(4), None);
	assert_eq!(source_clock(&evaluation, "src/b.rs"), (added, day(1)));
	assert_eq!(source_clock(&evaluation, "src/m/two.rs"), (matched, day(2)));
	assert!(matches!(live(&evaluation).sources[0].timing, Timing::Idle));
}
