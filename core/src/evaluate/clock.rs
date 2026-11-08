//! Clocks from git history: drift starts, new obligations and the suffix
//! minimum of committer dates.
//!
//! Everything here reads history through [`History`] alone. The building
//! block is [`trailing_runs`], which walks the first-parent chain back from a
//! commit once while many predicates are asked about each commit side by side.
//! [`drift`], [`drift_sources`] and [`introduced_at`] are built on it, and
//! [`RunStart::into_clock`] applies the timestamp rule with its
//! [`FUTURE_TOLERANCE`].
//!
//! This module dates the start of a clock and nothing more. Deadlines belong
//! to [`super::schedule`], and outcomes to the contexts of [`crate::policy`].

use std::collections::HashMap;

use chrono::{DateTime, TimeDelta, Utc};

use crate::ports::{
	BackendError, BackendErrorKind, Commit, CommitId, FirstParent, History,
	ObjectId, RepoPath, Revision,
};

#[cfg(test)]
mod tests;

/// How far after the evaluation instant a clock's timestamp may fall and
/// still be used as it is.
///
/// It keeps a runner whose clock lags a developer's from failing a fresh
/// push. A timestamp further in the future is a [`FutureDated`] error; the
/// evaluation instant is never substituted for it (H1).
pub const FUTURE_TOLERANCE: TimeDelta = TimeDelta::hours(24);

/// A condition asked of each commit on a first-parent walk, for
/// [`trailing_runs`].
///
/// The walk asks every running predicate about the same commit, and reads the
/// blobs that all of them need with one [`History::blobs_at`] call. A
/// predicate is not asked again once it has been false.
///
/// Mixed kinds of predicate can share one walk as `&mut dyn RunPredicate` or
/// `Box<dyn RunPredicate>`.
pub trait RunPredicate {
	/// The paths whose blobs [`RunPredicate::holds`] receives, in that order.
	/// They must stay the same for the whole walk.
	fn paths(&self) -> &[RepoPath];

	/// Whether the predicate holds at `commit`.
	///
	/// `blobs` has one entry for each of [`RunPredicate::paths`], in the same
	/// order: the blob at that path in `commit`'s tree, or `None` where the
	/// path is absent.
	///
	/// # Errors
	///
	/// Returns a backend failure, such as one met while reading a blob's
	/// content through [`History::blob`]. It ends the whole walk.
	fn holds(
		&mut self,
		commit: &Commit,
		blobs: &[Option<&ObjectId>],
	) -> Result<bool, BackendError>;
}

impl<T: RunPredicate + ?Sized> RunPredicate for &mut T {
	fn paths(&self) -> &[RepoPath] {
		(**self).paths()
	}

	fn holds(
		&mut self,
		commit: &Commit,
		blobs: &[Option<&ObjectId>],
	) -> Result<bool, BackendError> {
		(**self).holds(commit, blobs)
	}
}

impl<T: RunPredicate + ?Sized> RunPredicate for Box<T> {
	fn paths(&self) -> &[RepoPath] {
		(**self).paths()
	}

	fn holds(
		&mut self,
		commit: &Commit,
		blobs: &[Option<&ObjectId>],
	) -> Result<bool, BackendError> {
		(**self).holds(commit, blobs)
	}
}

/// A [`RunPredicate`] made of a closure and the paths it reads.
#[derive(Clone, Debug)]
pub struct FnPredicate<F> {
	paths: Vec<RepoPath>,
	predicate: F,
}

impl<F> FnPredicate<F>
where
	F: FnMut(&Commit, &[Option<&ObjectId>]) -> Result<bool, BackendError>,
{
	/// A predicate that calls `predicate` with each commit and its blobs at
	/// `paths`, as [`RunPredicate::holds`] describes.
	#[must_use]
	pub fn new(paths: Vec<RepoPath>, predicate: F) -> Self {
		Self { paths, predicate }
	}
}

impl<F> RunPredicate for FnPredicate<F>
where
	F: FnMut(&Commit, &[Option<&ObjectId>]) -> Result<bool, BackendError>,
{
	fn paths(&self) -> &[RepoPath] {
		&self.paths
	}

	fn holds(
		&mut self,
		commit: &Commit,
		blobs: &[Option<&ObjectId>],
	) -> Result<bool, BackendError> {
		(self.predicate)(commit, blobs)
	}
}

/// The oldest commit of a non-empty run, where a clock would start, before
/// the timestamp rule is applied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunStart {
	/// The oldest commit of the run.
	pub commit: CommitId,
	/// The minimum committer time over the first-parent commits from
	/// `commit` to the walk's first commit, both included.
	pub suffix_minimum: DateTime<Utc>,
}

impl RunStart {
	/// Applies the timestamp rule: the suffix minimum is the clock's timestamp,
	/// provided it is at most [`FUTURE_TOLERANCE`] after `at`, the
	/// evaluation instant.
	///
	/// # Errors
	///
	/// Returns [`FutureDated`], naming the commit, when the suffix minimum
	/// lies more than [`FUTURE_TOLERANCE`] after `at`.
	pub fn into_clock(
		self,
		at: DateTime<Utc>,
	) -> Result<ClockStart, FutureDated> {
		if self.suffix_minimum.signed_duration_since(at) > FUTURE_TOLERANCE {
			return Err(FutureDated {
				commit: self.commit,
				timestamp: self.suffix_minimum,
				instant: at,
			});
		}
		Ok(ClockStart {
			commit: self.commit,
			timestamp: self.suffix_minimum,
		})
	}
}

/// Where a predicate's trailing run ends. The run is the unbroken sequence of
/// first-parent commits, from the walk's first commit back, at which the
/// predicate holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Run {
	/// The predicate is false at the walk's first commit itself.
	Empty,
	/// The predicate holds from the walk's first commit back to
	/// [`RunStart::commit`], and is false at that commit's first parent.
	Bounded(RunStart),
	/// The predicate holds at every commit back to the root commit, which
	/// [`RunStart::commit`] names.
	ToRoot(RunStart),
}

/// A walk reached a shallow clone's boundary while a predicate still held, so
/// its run could not be concluded.
///
/// It is an operational error (exit 4) for that predicate alone. The report
/// pairs it with a recipe such as `fetch-depth: 0` or `git fetch
/// --unshallow`; `check` never fetches.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error(
	"the history is shallow: the search reached commit {boundary}, the oldest \
	 available, without concluding"
)]
pub struct ShallowHistory {
	/// The oldest commit available, whose first parent the shallow clone cut
	/// off.
	pub boundary: CommitId,
}

/// Walks the first-parent chain back from `from` once, and finds the
/// trailing run of every predicate in `predicates`.
///
/// Each predicate is asked about `from`, then about each first parent in
/// turn, until it is false or the history ends. Its run is the commits at
/// which it held, and its result is one of:
///
/// - [`Run::Empty`] when it is false at `from` itself;
/// - [`Run::Bounded`] with the run's oldest commit and suffix minimum;
/// - [`Run::ToRoot`] when it holds back to the root commit;
/// - [`ShallowHistory`] when it still holds at a commit whose first parent a
///   shallow clone cut off. A predicate that is false at that commit has
///   concluded and is unaffected.
///
/// Predicates stop independently, and the walk ends at the commit where the
/// last of them stops, so older history is never read. At each commit,
/// the blobs at the paths of all running predicates are read with a single
/// [`History::blobs_at`] call, which is skipped when they read no path.
/// Without predicates, nothing is read.
///
/// The result has one entry per predicate, in the order given.
///
/// # Errors
///
/// Fails as a whole when the backend fails, when a predicate fails, or when
/// the walk contradicts its own first-parent links: it ends early, yields a
/// commit other than the parent it announced, or returns the wrong number of
/// blobs.
pub fn trailing_runs<H, P>(
	history: &H,
	from: &CommitId,
	predicates: &mut [P],
) -> Result<Vec<Result<Run, ShallowHistory>>, BackendError>
where
	H: History,
	P: RunPredicate,
{
	if predicates.is_empty() {
		return Ok(Vec::new());
	}
	// Each predicate's index moves from `running` to `finished` exactly once,
	// so the sorted `finished` holds one result per predicate. A running
	// predicate carries the oldest commit of its run so far.
	let mut running: Vec<(usize, Option<RunStart>)> =
		(0..predicates.len()).map(|index| (index, None)).collect();
	let mut finished: Vec<(usize, Result<Run, ShallowHistory>)> =
		Vec::with_capacity(predicates.len());

	let mut walk = history.first_parent_walk(from);
	let mut expected = from.clone();
	let mut suffix_minimum: Option<DateTime<Utc>> = None;
	let mut lookup = BlobLookup::new(predicates, running.iter().map(|run| run.0));
	loop {
		let commit = next_commit(&mut walk, &expected)?;
		let minimum = suffix_minimum.map_or(commit.committer_time, |minimum| {
			minimum.min(commit.committer_time)
		});
		suffix_minimum = Some(minimum);

		let blobs = lookup.fetch(history, &commit.id)?;
		let mut view = Vec::new();
		let asked = running.len();
		let mut holding = Vec::with_capacity(asked);
		for (slot, (index, oldest)) in running.into_iter().enumerate() {
			lookup.view(slot, &blobs, &mut view);
			if predicates[index].holds(&commit, &view)? {
				let start = RunStart {
					commit: commit.id.clone(),
					suffix_minimum: minimum,
				};
				holding.push((index, start));
			} else {
				finished.push((index, Ok(oldest.map_or(Run::Empty, Run::Bounded))));
			}
		}
		if holding.is_empty() {
			break;
		}

		match commit.first_parent {
			FirstParent::Commit(parent) => {
				if holding.len() < asked {
					lookup = BlobLookup::new(predicates, holding.iter().map(|run| run.0));
				}
				running = holding
					.into_iter()
					.map(|(index, start)| (index, Some(start)))
					.collect();
				expected = parent;
			}
			FirstParent::Root => {
				finished.extend(
					holding
						.into_iter()
						.map(|(index, start)| (index, Ok(Run::ToRoot(start)))),
				);
				break;
			}
			FirstParent::Shallow => {
				finished.extend(holding.into_iter().map(|(index, _)| {
					let boundary = commit.id.clone();
					(index, Err(ShallowHistory { boundary }))
				}));
				break;
			}
		}
	}

	finished.sort_unstable_by_key(|(index, _)| *index);
	Ok(finished.into_iter().map(|(_, run)| run).collect())
}

/// The walk's next commit, which must be `expected`.
fn next_commit(
	walk: &mut impl Iterator<Item = Result<Commit, BackendError>>,
	expected: &CommitId,
) -> Result<Commit, BackendError> {
	let Some(next) = walk.next() else {
		return Err(BackendError::new(
			BackendErrorKind::MissingObject,
			format!("the first-parent walk ended before reaching commit {expected}"),
		));
	};
	let commit = next?;
	if commit.id != *expected {
		return Err(BackendError::new(
			BackendErrorKind::Git,
			format!(
				"the first-parent walk yielded commit {} where commit {expected} \
				 was expected",
				commit.id
			),
		));
	}
	Ok(commit)
}

/// The paths that the running predicates read, without duplicates, and where
/// each running predicate's own paths sit among them.
struct BlobLookup {
	/// The paths passed to [`History::blobs_at`].
	paths: Vec<RepoPath>,
	/// For each running predicate, in running order, the position in `paths`
	/// of each of its own paths.
	positions: Vec<Vec<usize>>,
}

impl BlobLookup {
	/// The lookup for the predicates at `running`, in that order.
	fn new<P: RunPredicate>(
		predicates: &[P],
		running: impl Iterator<Item = usize>,
	) -> Self {
		let mut known: HashMap<&RepoPath, usize> = HashMap::new();
		let mut paths = Vec::new();
		let mut positions = Vec::new();
		for index in running {
			let mut own = Vec::new();
			for path in predicates[index].paths() {
				let position = *known.entry(path).or_insert_with(|| {
					paths.push(path.clone());
					paths.len() - 1
				});
				own.push(position);
			}
			positions.push(own);
		}
		Self { paths, positions }
	}

	/// The blob at each of [`BlobLookup::paths`] in `commit`'s tree, read
	/// with one call, or with none when no path is needed.
	fn fetch<H: History>(
		&self,
		history: &H,
		commit: &CommitId,
	) -> Result<Vec<Option<ObjectId>>, BackendError> {
		if self.paths.is_empty() {
			return Ok(Vec::new());
		}
		let blobs = history.blobs_at(commit, &self.paths)?;
		if blobs.len() != self.paths.len() {
			return Err(BackendError::new(
				BackendErrorKind::Git,
				format!(
					"the backend returned {} blobs for {} paths at commit {commit}",
					blobs.len(),
					self.paths.len()
				),
			));
		}
		Ok(blobs)
	}

	/// Fills `view` with the blobs of the running predicate at `slot`, in the
	/// order of its own paths. `blobs` must come from
	/// [`BlobLookup::fetch`] on this lookup.
	fn view<'b>(
		&self,
		slot: usize,
		blobs: &'b [Option<ObjectId>],
		view: &mut Vec<Option<&'b ObjectId>>,
	) {
		view.clear();
		view.extend(
			self.positions[slot]
				.iter()
				.map(|&position| blobs[position].as_ref()),
		);
	}
}

/// Where a clock starts, with the timestamp that dates it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClockStart {
	/// The commit at which the clock starts.
	pub commit: CommitId,
	/// The suffix minimum of committer times from `commit` to the evaluated
	/// revision, which is at most [`FUTURE_TOLERANCE`] after the evaluation
	/// instant.
	pub timestamp: DateTime<Utc>,
}

/// A clock's timestamp would lie more than [`FUTURE_TOLERANCE`] after the
/// evaluation instant.
///
/// It is an operational error (exit 4) for that query alone. A later,
/// correctly dated commit on the checked line resolves it, because it lowers
/// the suffix minimum.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error(
	"the clock that starts at commit {commit} would be dated {timestamp}, more \
	 than {hours} hours after the evaluation instant {instant}; a later, \
	 correctly dated commit resolves this",
	hours = FUTURE_TOLERANCE.num_hours()
)]
pub struct FutureDated {
	/// The commit at which the clock would start.
	pub commit: CommitId,
	/// The suffix minimum of committer times from `commit` to the evaluated
	/// revision.
	pub timestamp: DateTime<Utc>,
	/// The evaluation instant.
	pub instant: DateTime<Utc>,
}

/// Why one query's clock could not be determined. Each is an operational
/// error (exit 4) that affects that query alone.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ClockError {
	/// The search reached a shallow boundary before concluding.
	#[error(transparent)]
	Shallow(#[from] ShallowHistory),
	/// The clock's timestamp lies too far after the evaluation instant.
	#[error(transparent)]
	FutureDated(#[from] FutureDated),
}

/// One attested source, for [`drift`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DriftQuery {
	/// The source's canonical path.
	pub path: RepoPath,
	/// The oid its attestation records.
	pub attested: ObjectId,
	/// Its canonical oid in the evaluated snapshot.
	pub current: ObjectId,
}

/// One attested source that the evaluated snapshot may lack, for
/// [`drift_sources`]: a source that exists, or an attested pattern member
/// that was deleted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceQuery {
	/// The source's canonical path.
	pub path: RepoPath,
	/// The oid its attestation records.
	pub attested: ObjectId,
	/// Its canonical oid in the evaluated snapshot, or `None` when the
	/// snapshot lacks the path. An absent path always differs from the
	/// attested oid.
	pub current: Option<ObjectId>,
}

/// What a drift search needs of a query.
trait Attested {
	/// The source's canonical path.
	fn source_path(&self) -> &RepoPath;
	/// The oid its attestation records.
	fn attested_oid(&self) -> &ObjectId;
	/// Its canonical oid in the evaluated snapshot, or `None` when absent.
	fn current_oid(&self) -> Option<&ObjectId>;
}

impl Attested for DriftQuery {
	fn source_path(&self) -> &RepoPath {
		&self.path
	}

	fn attested_oid(&self) -> &ObjectId {
		&self.attested
	}

	fn current_oid(&self) -> Option<&ObjectId> {
		Some(&self.current)
	}
}

impl Attested for SourceQuery {
	fn source_path(&self) -> &RepoPath {
		&self.path
	}

	fn attested_oid(&self) -> &ObjectId {
		&self.attested
	}

	fn current_oid(&self) -> Option<&ObjectId> {
		self.current.as_ref()
	}
}

/// Whether a source has drifted from its attestation, and when its clock
/// started.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Drift {
	/// The current oid equals the attested oid. No history was read for it.
	Fresh,
	/// Stale. The clock started at the first-parent commit after the newest
	/// one whose blob at the path equals the attested oid.
	Started(ClockStart),
	/// Stale, but the clock has not started: the committed snapshot's blob
	/// equals the attested oid and only the working tree differs, or the
	/// repository has no commits.
	///
	/// Under [`Revision::Commit`], which has no working tree, it arises only
	/// when [`DriftQuery::current`] or [`SourceQuery::current`] is not the
	/// commit's own blob.
	NotStarted,
	/// Stale, and the attested oid never appears in the first-parent history,
	/// which the walk covered to the root. The evaluation then reports
	/// `timing-unknown`, `stale` and `overdue`, with no grace or cadence
	/// deferral.
	TimingUnknown,
}

impl Drift {
	/// Whether the source is stale, which every variant but
	/// [`Drift::Fresh`] means.
	#[must_use]
	pub fn is_stale(&self) -> bool {
		!matches!(self, Self::Fresh)
	}
}

/// Finds each source's drift at `revision`, with one walk for all of them.
///
/// `at` is the evaluation instant, against which [`FUTURE_TOLERANCE`]
/// applies. The history is read back from `revision`'s commit: the commit
/// itself under [`Revision::Commit`], which has no working tree, or the
/// working tree's `HEAD`. Sources whose current oid equals the attested oid
/// need no history, so a call in which every source is fresh reads none.
///
/// Uncommitted changes never erase a clock already running against `HEAD`
/// (G4): with A attested, B committed and C in the working tree, the clock
/// starts at the commit that introduced B.
///
/// The result has one entry per query, in the order given. A query fails
/// alone when its search reaches a shallow boundary before concluding, or
/// when its timestamp is too far in the future.
///
/// # Errors
///
/// Fails as a whole when the backend fails or its walk is inconsistent, as
/// [`trailing_runs`] describes.
pub fn drift<H: History>(
	history: &H,
	revision: &Revision,
	at: DateTime<Utc>,
	queries: &[DriftQuery],
) -> Result<Vec<Result<Drift, ClockError>>, BackendError> {
	drift_of(history, revision, at, queries)
}

/// Finds each source's drift at `revision`, as [`drift`] does, for sources
/// that the evaluated snapshot may lack.
///
/// A source whose path is absent is stale, and is searched like any other:
/// its clock starts at the first-parent commit after the newest one whose
/// blob at the path equals the attested oid. A deletion is a later change,
/// so it never resets a clock that an earlier change started:
///
/// - a pattern member that matched its attestation until it was deleted
///   starts its clock at the deleting commit;
/// - a member that drifted before it was deleted keeps the clock of that
///   earlier drift;
/// - a deletion that exists only in the working tree has not started its
///   clock, and one whose attested oid never appears in the history is
///   [`Drift::TimingUnknown`].
///
/// # Errors
///
/// Fails as [`drift`] does.
pub fn drift_sources<H: History>(
	history: &H,
	revision: &Revision,
	at: DateTime<Utc>,
	queries: &[SourceQuery],
) -> Result<Vec<Result<Drift, ClockError>>, BackendError> {
	drift_of(history, revision, at, queries)
}

/// The search behind [`drift`] and [`drift_sources`].
fn drift_of<H: History, Q: Attested>(
	history: &H,
	revision: &Revision,
	at: DateTime<Utc>,
	queries: &[Q],
) -> Result<Vec<Result<Drift, ClockError>>, BackendError> {
	let is_fresh = |query: &Q| query.current_oid() == Some(query.attested_oid());
	// Without history, a fresh source is final and a stale one has not
	// started its clock. The walk below refines the stale ones.
	let mut outcomes: Vec<Result<Drift, ClockError>> = queries
		.iter()
		.map(|query| {
			Ok(if is_fresh(query) {
				Drift::Fresh
			} else {
				Drift::NotStarted
			})
		})
		.collect();
	let Some(head) = committed_head(revision) else {
		return Ok(outcomes);
	};

	let (walked, mut predicates): (Vec<usize>, Vec<Drifted<'_>>) = queries
		.iter()
		.enumerate()
		.filter(|(_, query)| !is_fresh(query))
		.map(|(index, query)| {
			let predicate = Drifted {
				path: query.source_path(),
				attested: query.attested_oid(),
			};
			(index, predicate)
		})
		.unzip();
	let runs = trailing_runs(history, head, &mut predicates)?;
	for (index, run) in walked.into_iter().zip(runs) {
		outcomes[index] = match run {
			Ok(Run::Empty) => Ok(Drift::NotStarted),
			Ok(Run::Bounded(start)) => start
				.into_clock(at)
				.map(Drift::Started)
				.map_err(ClockError::from),
			Ok(Run::ToRoot(_)) => Ok(Drift::TimingUnknown),
			Err(shallow) => Err(shallow.into()),
		};
	}
	Ok(outcomes)
}

/// Holds while the blob at a query's path differs from its attested oid,
/// including where the path is absent.
struct Drifted<'q> {
	path: &'q RepoPath,
	attested: &'q ObjectId,
}

impl RunPredicate for Drifted<'_> {
	fn paths(&self) -> &[RepoPath] {
		std::slice::from_ref(self.path)
	}

	fn holds(
		&mut self,
		_commit: &Commit,
		blobs: &[Option<&ObjectId>],
	) -> Result<bool, BackendError> {
		Ok(blobs != [Some(self.attested)])
	}
}

/// When a new obligation's clock starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ObligationStart {
	/// The clock started at the commit that introduced the obligation. When
	/// the obligation has existed since the root commit, it starts there.
	Started(ClockStart),
	/// The obligation is absent from the committed snapshot, so it exists
	/// only in the working tree and its clock has not started. This includes
	/// a repository without commits.
	///
	/// Under [`Revision::Commit`], which has no working tree, it arises only
	/// when the predicate is false at that commit.
	NotStarted,
}

/// Finds when each new obligation started, with one walk for all of them.
///
/// An obligation starts no earlier than the commit that introduced it: a
/// newly declared source, a newly added pattern, a file that began to match,
/// an unattested declaration, or a deleted match. Each predicate
/// says whether its obligation exists at a commit, for example "the
/// declaration is present", and the obligation starts at the oldest commit of
/// its trailing run. A run that reaches the root starts at the root commit,
/// which is not unknown timing.
///
/// `revision` and `at` mean what they mean for [`drift`]. The result has one
/// entry per predicate, in the order given, and a query fails alone as in
/// [`drift`].
///
/// # Errors
///
/// Fails as a whole when the backend or a predicate fails, or the walk is
/// inconsistent, as [`trailing_runs`] describes.
pub fn introduced_at<H, P>(
	history: &H,
	revision: &Revision,
	at: DateTime<Utc>,
	predicates: &mut [P],
) -> Result<Vec<Result<ObligationStart, ClockError>>, BackendError>
where
	H: History,
	P: RunPredicate,
{
	let Some(head) = committed_head(revision) else {
		return Ok(vec![Ok(ObligationStart::NotStarted); predicates.len()]);
	};
	let runs = trailing_runs(history, head, predicates)?;
	Ok(
		runs
			.into_iter()
			.map(|run| match run {
				Ok(Run::Empty) => Ok(ObligationStart::NotStarted),
				Ok(Run::Bounded(start) | Run::ToRoot(start)) => {
					Ok(ObligationStart::Started(start.into_clock(at)?))
				}
				Err(shallow) => Err(shallow.into()),
			})
			.collect(),
	)
}

/// The commit whose history a revision's clocks are read from, or `None` in
/// a repository without commits.
fn committed_head(revision: &Revision) -> Option<&CommitId> {
	match revision {
		Revision::Commit(commit) => Some(commit),
		Revision::WorkingTree { head } => head.as_ref(),
	}
}
