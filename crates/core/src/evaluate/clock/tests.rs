//! Tests of drift starts, new obligations and the timestamp rule against an
//! in-memory history.

use std::cell::{Cell, RefCell};

use chrono::{DateTime, TimeDelta, TimeZone, Utc};

use super::*;
use crate::testing::{FakeHistory, fake_oid, path};

/// Midnight UTC, `n` days after 1 January 2026.
fn day(n: i64) -> DateTime<Utc> {
	Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap() + TimeDelta::days(n)
}

fn working(history: &FakeHistory) -> Revision {
	Revision::WorkingTree {
		head: history.head(),
	}
}

fn query(file: &str, attested: &[u8], current: &[u8]) -> DriftQuery {
	DriftQuery {
		path: path(file),
		attested: fake_oid(attested),
		current: fake_oid(current),
	}
}

fn drift_one<H: History>(
	history: &H,
	revision: &Revision,
	at: DateTime<Utc>,
	query: DriftQuery,
) -> Result<Drift, ClockError> {
	let mut outcomes = drift(history, revision, at, &[query]).unwrap();
	assert_eq!(outcomes.len(), 1);
	outcomes.remove(0)
}

fn started(commit: &CommitId, timestamp: DateTime<Utc>) -> ClockStart {
	ClockStart {
		commit: commit.clone(),
		timestamp,
	}
}

/// A predicate that holds while `file` is present.
fn present(file: &str) -> impl RunPredicate + use<> {
	FnPredicate::new(vec![path(file)], |_commit, blobs| Ok(blobs[0].is_some()))
}

/// A history that records how it is read.
struct Recording {
	inner: FakeHistory,
	walked: Cell<usize>,
	lookups: RefCell<Vec<Vec<RepoPath>>>,
}

impl Recording {
	fn new(inner: FakeHistory) -> Self {
		Self {
			inner,
			walked: Cell::new(0),
			lookups: RefCell::new(Vec::new()),
		}
	}
}

impl History for Recording {
	fn first_parent_walk(
		&self,
		start: &CommitId,
	) -> impl Iterator<Item = Result<Commit, BackendError>> + '_ {
		self
			.inner
			.first_parent_walk(start)
			.inspect(|_| self.walked.set(self.walked.get() + 1))
	}

	fn blobs_at(
		&self,
		commit: &CommitId,
		paths: &[RepoPath],
	) -> Result<Vec<Option<ObjectId>>, BackendError> {
		self.lookups.borrow_mut().push(paths.to_vec());
		self.inner.blobs_at(commit, paths)
	}

	fn blob(&self, id: &ObjectId) -> Result<Option<Vec<u8>>, BackendError> {
		self.inner.blob(id)
	}

	fn files_ever_in(
		&self,
		start: &CommitId,
		directory: &RepoPath,
	) -> Result<Vec<RepoPath>, BackendError> {
		self.inner.files_ever_in(start, directory)
	}
}

/// How a [`Faulty`] history breaks the backend contract.
#[derive(Clone, Copy)]
enum Fault {
	/// The walk stops after its first commit, which announced a parent.
	EndsEarly,
	/// The walk skips the first commit's parent.
	SkipsParent,
	/// `blobs_at` drops the last blob.
	DropsBlob,
}

/// A history whose backend breaks its contract.
struct Faulty {
	inner: FakeHistory,
	fault: Fault,
}

impl History for Faulty {
	fn first_parent_walk(
		&self,
		start: &CommitId,
	) -> impl Iterator<Item = Result<Commit, BackendError>> + '_ {
		let walk = self.inner.first_parent_walk(start);
		let keep = |position: &usize| match self.fault {
			Fault::EndsEarly => *position == 0,
			Fault::SkipsParent => *position != 1,
			Fault::DropsBlob => true,
		};
		walk
			.enumerate()
			.filter(move |(position, _)| keep(position))
			.map(|(_, commit)| commit)
	}

	fn blobs_at(
		&self,
		commit: &CommitId,
		paths: &[RepoPath],
	) -> Result<Vec<Option<ObjectId>>, BackendError> {
		let mut blobs = self.inner.blobs_at(commit, paths)?;
		if matches!(self.fault, Fault::DropsBlob) {
			blobs.pop();
		}
		Ok(blobs)
	}

	fn blob(&self, id: &ObjectId) -> Result<Option<Vec<u8>>, BackendError> {
		self.inner.blob(id)
	}

	fn files_ever_in(
		&self,
		start: &CommitId,
		directory: &RepoPath,
	) -> Result<Vec<RepoPath>, BackendError> {
		self.inner.files_ever_in(start, directory)
	}
}

#[test]
fn a_backend_that_breaks_its_contract_fails_the_whole_call() {
	let mut inner = FakeHistory::new();
	inner.commit(day(0), &[("src/a.rs", Some(b"A"))]);
	inner.commit(day(1), &[("src/a.rs", Some(b"B"))]);
	inner.commit(day(2), &[("src/a.rs", Some(b"C"))]);
	let queries = [query("src/a.rs", b"A", b"C")];

	for (fault, kind) in [
		(Fault::EndsEarly, BackendErrorKind::MissingObject),
		(Fault::SkipsParent, BackendErrorKind::Git),
		(Fault::DropsBlob, BackendErrorKind::Git),
	] {
		let history = Faulty {
			inner: inner.clone(),
			fault,
		};
		let revision = working(&history.inner);
		let failure = drift(&history, &revision, day(3), &queries).unwrap_err();
		assert_eq!(failure.kind, kind);
	}
}

#[test]
fn a_revert_restores_freshness_so_the_clock_starts_after_it() {
	let mut history = FakeHistory::new();
	history.commit(day(0), &[("src/a.rs", Some(b"A"))]);
	history.commit(day(1), &[("src/a.rs", Some(b"B"))]);
	history.commit(day(2), &[("src/a.rs", Some(b"A"))]);
	let c = history.commit(day(3), &[("src/a.rs", Some(b"C"))]);

	let outcome = drift_one(
		&history,
		&working(&history),
		day(4),
		query("src/a.rs", b"A", b"C"),
	);
	assert_eq!(outcome, Ok(Drift::Started(started(&c, day(3)))));
}

#[test]
fn an_attested_oid_missing_from_history_is_timing_unknown() {
	// The April merge: the attestation of A arrived from a merged branch, so
	// A never appears on the first-parent chain. Falling back to the commit
	// that introduced the attestation would grant grace to old drift.
	let mut history = FakeHistory::new();
	history.commit(day(0), &[("src/a.rs", Some(b"B"))]);
	history.commit(day(30), &[("src/a.rs", Some(b"C"))]);
	history.commit(day(90), &[("docs/a.md", Some(b"attests A"))]);

	let outcome = drift_one(
		&history,
		&working(&history),
		day(100),
		query("src/a.rs", b"A", b"C"),
	);
	assert_eq!(outcome, Ok(Drift::TimingUnknown));
	assert!(Drift::TimingUnknown.is_stale());
}

#[test]
fn an_attested_oid_that_was_never_committed_is_timing_unknown() {
	// X is committed, A was attested in the working tree, and the working
	// tree has since moved on to C. The history is read literally: A never
	// appears on the first-parent chain. The report lists this reading.
	let mut history = FakeHistory::new();
	history.commit(day(0), &[("src/a.rs", Some(b"X"))]);

	let outcome = drift_one(
		&history,
		&working(&history),
		day(1),
		query("src/a.rs", b"A", b"C"),
	);
	assert_eq!(outcome, Ok(Drift::TimingUnknown));
}

#[test]
fn a_later_correctly_dated_commit_corrects_a_future_dated_drift_commit() {
	let mut history = FakeHistory::new();
	history.commit(day(0), &[("src/a.rs", Some(b"A"))]);
	let drifted = history.commit(day(400), &[("src/a.rs", Some(b"B"))]);

	// Alone, the future-dated commit cannot date the clock, and the
	// evaluation instant is not substituted for it.
	let outcome = drift_one(
		&history,
		&Revision::Commit(drifted.clone()),
		day(5),
		query("src/a.rs", b"A", b"B"),
	);
	assert_eq!(
		outcome,
		Err(ClockError::FutureDated(FutureDated {
			commit: drifted.clone(),
			timestamp: day(400),
			instant: day(5),
		}))
	);

	// A later commit dated correctly bounds when the drift landed.
	history.commit(day(10), &[("README.md", Some(b"fix"))]);
	let outcome = drift_one(
		&history,
		&working(&history),
		day(11),
		query("src/a.rs", b"A", b"B"),
	);
	assert_eq!(outcome, Ok(Drift::Started(started(&drifted, day(10)))));
}

#[test]
fn a_future_dated_head_within_the_tolerance_is_used_as_it_is() {
	let at = day(10);
	for ahead in [TimeDelta::hours(23), FUTURE_TOLERANCE] {
		let mut history = FakeHistory::new();
		history.commit(day(0), &[("src/a.rs", Some(b"A"))]);
		let head = history.commit(at + ahead, &[("src/a.rs", Some(b"B"))]);

		let outcome = drift_one(
			&history,
			&working(&history),
			at,
			query("src/a.rs", b"A", b"B"),
		);
		assert_eq!(outcome, Ok(Drift::Started(started(&head, at + ahead))));
	}
}

#[test]
fn a_future_dated_head_beyond_the_tolerance_is_an_error() {
	let at = day(10);
	let ahead = FUTURE_TOLERANCE + TimeDelta::seconds(1);
	let mut history = FakeHistory::new();
	history.commit(day(0), &[("src/a.rs", Some(b"A"))]);
	let head = history.commit(at + ahead, &[("src/a.rs", Some(b"B"))]);

	let outcome = drift_one(
		&history,
		&working(&history),
		at,
		query("src/a.rs", b"A", b"B"),
	);
	let error = FutureDated {
		commit: head,
		timestamp: at + ahead,
		instant: at,
	};
	assert!(error.to_string().contains("24 hours"));
	assert_eq!(outcome, Err(ClockError::FutureDated(error)));
}

#[test]
fn a_shallow_boundary_fails_only_the_searches_it_interrupts() {
	let mut history = FakeHistory::new();
	history.commit(day(0), &[("src/a.rs", Some(b"A"))]);
	let boundary = history.commit(day(1), &[("src/a.rs", Some(b"B"))]);
	let c = history.commit(day(2), &[("src/a.rs", Some(b"C"))]);
	let d = history.commit(
		day(3),
		&[("src/a.rs", Some(b"D")), ("src/b.rs", Some(b"new"))],
	);
	history.make_shallow_at(&boundary);

	let outcomes = drift(
		&history,
		&working(&history),
		day(4),
		&[
			// A lies beyond the boundary, so the search cannot conclude.
			query("src/a.rs", b"A", b"D"),
			// B is the boundary commit itself, which concludes the search.
			query("src/a.rs", b"B", b"D"),
			// C is found before the boundary.
			query("src/a.rs", b"C", b"D"),
		],
	)
	.unwrap();
	assert_eq!(
		outcomes,
		vec![
			Err(ClockError::Shallow(ShallowHistory {
				boundary: boundary.clone(),
			})),
			Ok(Drift::Started(started(&c, day(2)))),
			Ok(Drift::Started(started(&d, day(3)))),
		]
	);

	// An obligation present since before the boundary cannot be dated
	// either, whereas one introduced after it can.
	let mut predicates = [present("src/a.rs"), present("src/b.rs")];
	let outcomes =
		introduced_at(&history, &working(&history), day(4), &mut predicates)
			.unwrap();
	assert_eq!(
		outcomes,
		vec![
			Err(ClockError::Shallow(ShallowHistory { boundary })),
			Ok(ObligationStart::Started(started(&d, day(3)))),
		]
	);
}

#[test]
fn an_unborn_repository_has_no_started_clocks() {
	let history = FakeHistory::new();
	let revision = Revision::WorkingTree { head: None };

	let outcomes = drift(
		&history,
		&revision,
		day(0),
		&[query("src/a.rs", b"A", b"B"), query("src/b.rs", b"A", b"A")],
	)
	.unwrap();
	assert_eq!(outcomes, vec![Ok(Drift::NotStarted), Ok(Drift::Fresh)]);

	let mut predicates = [present("docs/a.md")];
	let outcomes =
		introduced_at(&history, &revision, day(0), &mut predicates).unwrap();
	assert_eq!(outcomes, vec![Ok(ObligationStart::NotStarted)]);
}

#[test]
fn drift_that_is_only_uncommitted_has_not_started_its_clock() {
	let mut history = FakeHistory::new();
	history.commit(day(0), &[("src/a.rs", Some(b"A"))]);

	let outcome = drift_one(
		&history,
		&working(&history),
		day(1),
		query("src/a.rs", b"A", b"B"),
	);
	assert_eq!(outcome, Ok(Drift::NotStarted));
	assert!(Drift::NotStarted.is_stale());
}

#[test]
fn an_uncommitted_change_never_erases_a_running_clock() {
	let mut history = FakeHistory::new();
	history.commit(day(0), &[("src/a.rs", Some(b"A"))]);
	let b = history.commit(day(1), &[("src/a.rs", Some(b"B"))]);

	// A attested, B committed and C in the working tree.
	let outcome = drift_one(
		&history,
		&working(&history),
		day(2),
		query("src/a.rs", b"A", b"C"),
	);
	assert_eq!(outcome, Ok(Drift::Started(started(&b, day(1)))));
}

#[test]
fn a_batch_gives_the_same_answers_as_single_queries() {
	let mut history = FakeHistory::new();
	history.commit(
		day(0),
		&[
			("src/a.rs", Some(b"A1")),
			("src/b.rs", Some(b"B1")),
			("src/c.rs", Some(b"C1")),
		],
	);
	history.commit(day(1), &[("src/a.rs", Some(b"A2"))]);
	history.commit(day(4), &[("src/c.rs", Some(b"C2"))]);
	history.commit(day(3), &[("src/c.rs", Some(b"C3"))]);
	history.commit(day(5), &[("src/b.rs", None)]);
	let revision = working(&history);
	let queries = [
		query("src/a.rs", b"A1", b"A2"),
		query("src/b.rs", b"B1", b"B1"),
		query("src/c.rs", b"C1", b"C3"),
		query("src/c.rs", b"C2", b"C3"),
		query("src/a.rs", b"A0", b"A2"),
		query("src/b.rs", b"B1", b"B2"),
		query("src/d.rs", b"D1", b"D2"),
	];

	let batch = drift(&history, &revision, day(6), &queries).unwrap();
	let single: Vec<_> = queries
		.iter()
		.map(|query| drift_one(&history, &revision, day(6), query.clone()))
		.collect();
	assert_eq!(batch, single);
	assert!(matches!(batch[0], Ok(Drift::Started(_))));
	assert_eq!(batch[1], Ok(Drift::Fresh));
	assert!(matches!(batch[4], Ok(Drift::TimingUnknown)));
}

#[test]
fn the_timestamp_is_the_suffix_minimum_up_to_the_evaluated_revision() {
	let mut history = FakeHistory::new();
	history.commit(day(0), &[("src/a.rs", Some(b"A"))]);
	let b = history.commit(day(5), &[("src/a.rs", Some(b"B"))]);
	history.commit(day(3), &[("README.md", Some(b"one"))]);
	history.commit(day(4), &[("README.md", Some(b"two"))]);

	let outcome = drift_one(
		&history,
		&working(&history),
		day(6),
		query("src/a.rs", b"A", b"B"),
	);
	assert_eq!(outcome, Ok(Drift::Started(started(&b, day(3)))));

	// Under `--rev`, commits after the evaluated one are not part of the
	// suffix.
	let outcome = drift_one(
		&history,
		&Revision::Commit(b.clone()),
		day(6),
		query("src/a.rs", b"A", b"B"),
	);
	assert_eq!(outcome, Ok(Drift::Started(started(&b, day(5)))));
}

#[test]
fn a_deleted_source_counts_as_drift() {
	let mut history = FakeHistory::new();
	history.commit(day(0), &[("src/a.rs", Some(b"A"))]);
	let deleted = history.commit(day(1), &[("src/a.rs", None)]);
	history.commit(day(2), &[("src/a.rs", Some(b"B"))]);

	let outcome = drift_one(
		&history,
		&working(&history),
		day(3),
		query("src/a.rs", b"A", b"B"),
	);
	assert_eq!(outcome, Ok(Drift::Started(started(&deleted, day(1)))));
}

#[test]
fn fresh_sources_read_no_history() {
	// The revision names a commit the history lacks, so any walk would fail.
	let history = FakeHistory::new();
	let revision = Revision::Commit(CommitId(fake_oid(b"unknown commit")));

	let outcomes = drift(
		&history,
		&revision,
		day(0),
		&[query("src/a.rs", b"A", b"A")],
	)
	.unwrap();
	assert_eq!(outcomes, vec![Ok(Drift::Fresh)]);

	let failure = drift(
		&history,
		&revision,
		day(0),
		&[query("src/a.rs", b"A", b"B")],
	)
	.unwrap_err();
	assert_eq!(failure.kind, BackendErrorKind::MissingObject);
}

#[test]
fn a_declaration_present_since_the_root_starts_at_the_root() {
	let mut history = FakeHistory::new();
	let root = history.commit(day(2), &[("docs/a.md", Some(b"declared"))]);
	history.commit(day(1), &[("src/a.rs", Some(b"A"))]);
	history.commit(day(3), &[("src/b.rs", Some(b"B"))]);

	let mut predicates = [present("docs/a.md")];
	let outcomes =
		introduced_at(&history, &working(&history), day(4), &mut predicates)
			.unwrap();
	assert_eq!(
		outcomes,
		vec![Ok(ObligationStart::Started(started(&root, day(1))))]
	);
}

#[test]
fn an_obligation_starts_at_the_commit_that_last_introduced_it() {
	let mut history = FakeHistory::new();
	history.commit(day(0), &[("docs/a.md", Some(b"no declaration"))]);
	history.commit(day(1), &[("docs/a.md", Some(b"source-sentry: on"))]);
	history.commit(day(2), &[("docs/a.md", Some(b"no declaration"))]);
	let added =
		history.commit(day(3), &[("docs/a.md", Some(b"source-sentry: on"))]);
	history.commit(day(4), &[("src/a.rs", Some(b"A"))]);

	// The predicate reads the document's content, as a declaration check
	// would.
	let declared = FnPredicate::new(vec![path("docs/a.md")], |_commit, blobs| {
		let Some(oid) = blobs[0] else {
			return Ok(false);
		};
		let content = history.blob(oid)?.unwrap_or_default();
		Ok(content.starts_with(b"source-sentry"))
	});
	// Mixed kinds of predicate share one walk.
	let mut predicates: Vec<Box<dyn RunPredicate + '_>> = vec![
		Box::new(declared),
		Box::new(present("src/a.rs")),
		Box::new(present("src/missing.rs")),
	];
	let outcomes =
		introduced_at(&history, &working(&history), day(5), &mut predicates)
			.unwrap();
	assert_eq!(
		outcomes,
		vec![
			Ok(ObligationStart::Started(started(&added, day(3)))),
			Ok(ObligationStart::Started(started(
				&history.head().unwrap(),
				day(4)
			))),
			Ok(ObligationStart::NotStarted),
		]
	);
}

#[test]
fn an_obligation_starting_too_far_ahead_is_an_error() {
	let mut history = FakeHistory::new();
	let head = history.commit(day(30), &[("docs/a.md", Some(b"declared"))]);

	let mut predicates = [present("docs/a.md")];
	let outcomes =
		introduced_at(&history, &working(&history), day(1), &mut predicates)
			.unwrap();
	assert_eq!(
		outcomes,
		vec![Err(ClockError::FutureDated(FutureDated {
			commit: head,
			timestamp: day(30),
			instant: day(1),
		}))]
	);
}

#[test]
fn the_walk_reads_each_commit_once_and_stops_when_every_run_has_ended() {
	let mut fake = FakeHistory::new();
	for n in 0..6 {
		let content = format!("version {n}");
		fake.commit(day(n), &[("src/a.rs", Some(content.as_bytes()))]);
	}
	fake.commit(day(6), &[("src/b.rs", Some(b"B"))]);
	let history = Recording::new(fake);
	let head = history.inner.head().unwrap();

	let unchanged_since = fake_oid(b"version 2");
	let mut predicates: Vec<Box<dyn RunPredicate>> = vec![
		// False at the head: an empty run.
		Box::new(present("src/c.rs")),
		// Holds at the head only, where `src/b.rs` was added.
		Box::new(present("src/b.rs")),
		// Holds back to commit 3, the oldest without version 2.
		Box::new(FnPredicate::new(
			vec![path("src/a.rs"), path("src/b.rs")],
			move |_commit, blobs| Ok(blobs[0] != Some(&unchanged_since)),
		)),
	];
	let runs = trailing_runs(&history, &head, &mut predicates).unwrap();

	assert_eq!(runs[0], Ok(Run::Empty));
	assert_eq!(
		runs[1],
		Ok(Run::Bounded(RunStart {
			commit: head,
			suffix_minimum: day(6),
		}))
	);
	assert!(matches!(
		&runs[2],
		Ok(Run::Bounded(start)) if start.suffix_minimum == day(3)
	));
	// Commits 6 down to 2: the walk stops where the last run ends, before
	// the root.
	assert_eq!(history.walked.get(), 5);
	// One lookup per commit, covering only the predicates still running.
	let lookups: Vec<Vec<RepoPath>> = history
		.lookups
		.borrow()
		.iter()
		.map(|paths| {
			let mut paths = paths.clone();
			paths.sort();
			paths
		})
		.collect();
	let all = vec![path("src/a.rs"), path("src/b.rs"), path("src/c.rs")];
	let a_and_b = vec![path("src/a.rs"), path("src/b.rs")];
	assert_eq!(
		lookups,
		vec![
			all,
			a_and_b.clone(),
			a_and_b.clone(),
			a_and_b.clone(),
			a_and_b,
		]
	);
}

#[test]
fn a_run_reaching_the_root_is_reported_as_such() {
	let mut history = FakeHistory::new();
	let root = history.commit(day(1), &[("src/a.rs", Some(b"A"))]);
	let head = history.commit(day(0), &[("src/a.rs", Some(b"B"))]);

	let mut predicates = [present("src/a.rs")];
	let runs = trailing_runs(&history, &head, &mut predicates).unwrap();
	assert_eq!(
		runs,
		vec![Ok(Run::ToRoot(RunStart {
			commit: root,
			suffix_minimum: day(0),
		}))]
	);

	let mut none: Vec<Box<dyn RunPredicate>> = Vec::new();
	assert_eq!(trailing_runs(&history, &head, &mut none).unwrap(), vec![]);
}

#[test]
fn a_failing_predicate_fails_the_whole_walk() {
	let mut history = FakeHistory::new();
	let head = history.commit(day(0), &[("src/a.rs", Some(b"A"))]);

	let mut predicates: Vec<Box<dyn RunPredicate>> = vec![
		Box::new(present("src/a.rs")),
		Box::new(FnPredicate::new(vec![], |_commit, _blobs| {
			Err(BackendError::new(BackendErrorKind::Io, "unreadable"))
		})),
	];
	let failure = trailing_runs(&history, &head, &mut predicates).unwrap_err();
	assert_eq!(failure.kind, BackendErrorKind::Io);
}

/// A deleted source, for `drift_sources`.
fn deleted(file: &str, attested: &[u8]) -> SourceQuery {
	SourceQuery {
		path: path(file),
		attested: fake_oid(attested),
		current: None,
	}
}

/// A deleted source is searched as drift. One that
/// matched its attestation until the deletion starts there, one that drifted
/// earlier keeps its earlier clock, and an attested oid that never appears is
/// unknown timing.
#[test]
fn a_deleted_source_drifts_from_its_last_attested_commit() {
	let mut history = FakeHistory::new();
	history.commit(
		day(0),
		&[("src/a.rs", Some(b"A")), ("src/b.rs", Some(b"B"))],
	);
	let changed = history.commit(day(2), &[("src/b.rs", Some(b"B2"))]);
	let removal =
		history.commit(day(5), &[("src/a.rs", None), ("src/b.rs", None)]);
	let revision = working(&history);
	let queries = [
		deleted("src/a.rs", b"A"),
		deleted("src/b.rs", b"B"),
		deleted("src/c.rs", b"C"),
	];
	let outcomes = drift_sources(&history, &revision, day(6), &queries).unwrap();
	assert_eq!(
		outcomes,
		vec![
			Ok(Drift::Started(started(&removal, day(5)))),
			Ok(Drift::Started(started(&changed, day(2)))),
			Ok(Drift::TimingUnknown),
		]
	);
}

/// A deletion that exists only in the working tree has not started
/// its clock, and a present source behaves as it does for `drift`.
#[test]
fn an_uncommitted_deletion_has_not_started() {
	let mut history = FakeHistory::new();
	history.commit(day(0), &[("src/a.rs", Some(b"A"))]);
	let revision = working(&history);
	let queries = [
		deleted("src/a.rs", b"A"),
		SourceQuery {
			path: path("src/a.rs"),
			attested: fake_oid(b"A"),
			current: Some(fake_oid(b"A")),
		},
	];
	let outcomes = drift_sources(&history, &revision, day(1), &queries).unwrap();
	assert_eq!(outcomes, vec![Ok(Drift::NotStarted), Ok(Drift::Fresh)]);
}
