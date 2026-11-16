//! The commits that changed one path, as `git log` tells them, for a
//! reviewer who asks what happened to a source since its review.

use source_sentry_core::ports::{BackendError, CommitId, RepoPath};

use crate::repository::GitRepository;

/// One commit that changed a path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathCommit {
	/// The commit's full object name, in hexadecimal.
	pub id: String,
	/// Its committer date, as `YYYY-MM-DD`.
	pub date: String,
	/// Its author's name.
	pub author: String,
	/// The first line of its message.
	pub subject: String,
}

/// The commits that changed a path, newest first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PathCommits {
	/// The commits, at most as many as were asked for.
	pub commits: Vec<PathCommit>,
	/// Whether more commits changed the path than `commits` holds.
	pub more: bool,
}

impl GitRepository {
	/// The commits reachable from `HEAD` that changed `path`, newest first,
	/// and at most `limit` of them.
	///
	/// With `since`, the list starts at that commit: it leaves out what the
	/// commit's first parent already held, so a change that a merge brought
	/// is listed with the commits of its branch. Without it, or when `since`
	/// has no parent, the whole history of the path is listed. `path` is
	/// taken literally, never as a pattern.
	///
	/// # Errors
	///
	/// Fails when `HEAD` cannot be read, or when `git log` cannot run or
	/// fails.
	pub fn path_commits(
		&self,
		path: &RepoPath,
		since: Option<&CommitId>,
		limit: usize,
	) -> Result<PathCommits, BackendError> {
		if self.head()?.is_none() {
			return Ok(PathCommits::default());
		}
		let parent = since
			.and_then(|commit| self.git_oid(&commit.0))
			.and_then(|oid| self.raw().find_commit(oid).ok())
			.and_then(|commit| commit.parent_id(0).ok())
			.map(|parent| format!("^{parent}"));
		// One more than asked for tells whether the list was cut.
		let count = format!("--max-count={}", limit.saturating_add(1));
		let mut args = vec![
			"--literal-pathspecs",
			"log",
			"-z",
			"--date=short",
			// Fields are separated, and with `-z` records ended, by NUL, the one
			// byte that a commit's metadata cannot hold.
			"--format=%H%x00%cd%x00%an%x00%s",
			count.as_str(),
			"HEAD",
		];
		args.extend(parent.as_deref());
		args.extend(["--", path.as_str()]);
		let output = self.git().output(&args, None)?;
		let fields: Vec<&[u8]> = output
			.strip_suffix(&[0])
			.map(|fields| fields.split(|&byte| byte == 0).collect())
			.unwrap_or_default();
		let text = |field: &[u8]| String::from_utf8_lossy(field).into_owned();
		let (records, _) = fields.as_chunks::<4>();
		let mut commits: Vec<PathCommit> = records
			.iter()
			.map(|[id, date, author, subject]| PathCommit {
				id: text(id).trim_matches('\n').to_owned(),
				date: text(date),
				author: text(author),
				subject: text(subject),
			})
			.collect();
		let more = commits.len() > limit;
		commits.truncate(limit);
		Ok(PathCommits { commits, more })
	}
}
