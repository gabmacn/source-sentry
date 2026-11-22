# Changelog

All notable changes to Source Sentry are recorded in this file. The format
follows [Keep a Changelog 1.1.0](https://keepachangelog.com/en/1.1.0/), and
versions follow [Semantic Versioning 2.0.0](https://semver.org/spec/v2.0.0.html).
The three crates, `source-sentry`, `source-sentry-core` and
`source-sentry-git`, are released together under one version.

## How this file is written

- Changes that have not been released yet go under `## [Unreleased]`.
- A release turns that heading into `## [<version>] - <YYYY-MM-DD>`, for
  example `## [0.1.0] - 2026-11-20`, and starts a new, empty `Unreleased`
  section above it.
- Within a section, changes are grouped under `### Added`, `### Changed`,
  `### Deprecated`, `### Removed`, `### Fixed` and `### Security`, in that
  order. Empty groups are left out.
- Each change is one bullet point, written for users rather than for
  contributors.

The notes of each release on the Forgejo releases page are taken verbatim from
that version's section, so a release cannot be made without one.

## [Unreleased]

### Added

- `impact` lists the documents that describe given files: those whose
  sources name them, and those that cite those documents in turn, with
  sealed records listed as unaffected. It takes paths, `-` for paths on
  standard input (`-z` for NUL-separated ones), `--changed`, `--staged` and
  `--rev`, and writes text or JSON (`--format json`). It reads declarations
  only, searches the starting project and its nested projects, says which
  projects it did not search, and is not a gate.
- `explain` shows the documents that cite a file. A document or unclaimed
  file gains a "Cited by" section, and a file that is no document but that
  documents cite is explained as a source rather than refused.

### Changed

- Dates and instants are now read and written with `jiff` instead of
  `chrono`, which has been declared unmaintained (RUSTSEC-2026-0543).
  Every instant Source Sentry has written reads back as before, unless its
  year is negative, but hand-written instants in lockfiles, frontmatter
  and `check --at` follow `jiff`'s rules. They may now use an offset
  without a colon (`+0100`), a comma before the fraction, no seconds
  (`2026-11-16T23:46Z`), or a bracketed time zone after the offset
  (`2026-11-16T23:46:37Z[Europe/Madrid]`), where the offset alone decides
  the instant. They may no longer have more than nine fractional digits,
  spaces around the value, `UTC` in place of an offset, or a signed year
  of fewer than six digits, such as `-0001`. A leap second, `23:59:60`, is
  read as `23:59:59`.
- An instant with a fraction of a second is written without trailing zeros,
  `2026-11-16T23:46:37.01Z` rather than `2026-11-16T23:46:37.010Z`, in
  lockfiles, JSON reports and frontmatter. Source Sentry records whole
  seconds, so only instants written by hand are affected.
- The messages for sealing a record that is already sealed, and for a clock
  whose commit is dated in the future, write instants as
  `2026-11-16T23:46:37Z`, as files do, rather than
  `2026-11-16 23:46:37 UTC`.
- A cadence's `dtstart`, in a config, in frontmatter or in a lockfile, and
  the start date of `track --cadence`, must be written exactly
  `YYYY-MM-DD`. Single-digit months and days, as in `2026-1-8`, and a sign,
  as in `+2026-12-01`, are no longer read. A value with a time of day, such
  as `2026-12-01T10:00:00`, is refused rather than cut down to its date.
- Dates must fall within the years -9999 to 9999, and instants within
  `-009999-01-02T01:59:59Z` to `9999-12-30T22:00:00Z`. A commit dated
  outside that range is an error, and a cadence whose next due date would
  fall after 31 December 9999 is a `date-out-of-range` error, where it used
  to suggest a start date that could not be written.
- The `source-sentry-core` and `source-sentry-git` crates use
  `jiff::Timestamp` and `jiff::civil::Date` in their interfaces instead of
  `chrono`'s `DateTime<Utc>` and `NaiveDate`. `TimeZone::local_date` and
  `TimeZone::to_local` no longer return a `Result`, `to_local` returns a
  `jiff::Zoned`, and `HORIZON_DAYS` is a `u32`. `model::parse_date` reads a
  `YYYY-MM-DD` date, and `model::whole_second` drops the fraction of a
  second from an instant.
- A cadence's rule no longer accepts `BYWEEKDAY`, a synonym of `BYDAY`
  that RFC 5545 does not have. A rule that uses it is a configuration
  error whose message says to write `BYDAY`; the occurrences are the
  same.

- Configs, presets and declarations in frontmatter refuse a bare number or
  boolean where text belongs, such as `owners: [123]`, and a quoted number
  or boolean where one belongs, such as `grace: '2'` or `root: 'true'`,
  instead of converting it silently. The message says how to quote the
  value.
- A review, completion, seal or rename entry that leaves out `by` is refused
  as malformed instead of being read as `by: null`. The value may still be
  `null`.
- A cadence's numbered `BYDAY` days are limited to -4 to 5 with
  `FREQ=MONTHLY` and to -52 to 53 with `FREQ=YEARLY`, as before, but the
  limits are now Source Sentry's own and documented, and a rule outside them
  is refused with Source Sentry's own message instead of the recurrence
  library's.
- While the window is too small for the full screen of `review`, only `q`
  and `Ctrl+C` act; other keys, the wheel and clicks no longer record or
  open anything unseen, and a held answer or an open box is still there when
  the window grows.
- In the full screen of `review`, a key it does not use, such as F1, Home or
  Delete, withdraws a held answer, as any other key does, instead of leaving
  it to be recorded by a later Enter.
- The list of this session's answers that `u` opens in `review` shows every
  answer that still stands, not just the newest ten, and scrolls inside its
  box with the arrow keys, Page Up, Page Down and the wheel, as the
  confirmations do.
- `Evaluation` in the `source-sentry-core` crate has a public field,
  `cited`, holding the documents of other projects that the evaluated
  documents cite, and a method, `upstream_subjects_of`.
- The `History` trait of the `source-sentry-core` crate replaces
  `files_ever_in`, which walked the whole history, with `blobs_in`, which
  lists the files of one directory in one commit with their blob IDs. The
  `source-sentry-git` crate adds `GitRepository::open_from_env`, which finds
  the repository as git does; `GitRepository::open` still ignores the
  environment.
- `check` reports a remote identity that owner lockfiles of two projects
  track at once as `double-claim`, in each of the projects, and `track`
  refuses to track an identity that another project already tracks. Moving
  it between projects with `track --from/--to` is unaffected.
- A `renamed` list in a remote owner lockfile is refused
  (`invalid-lockfile`), as `seal` and `completed` are.
- `Inventory` in the `source-sentry-core` crate has a public field,
  `elsewhere`, holding the remote identities that other projects' owner
  lockfiles track.
- Source Sentry uses the repository, working tree and index that git would
  use from the same directory and environment, so `GIT_DIR`,
  `GIT_WORK_TREE`, `core.worktree`, `GIT_INDEX_FILE`, `GIT_OBJECT_DIRECTORY`
  and `GIT_ALTERNATE_OBJECT_DIRECTORIES` apply, and a printed `git add`
  stages the file in the repository that was changed.
- Started outside the working tree that `GIT_WORK_TREE` names, relative
  paths start at the working tree's root and are shown from it, as git takes
  them, so printed `git` and `source-sentry` commands run as printed.
- In a `pre-commit` hook, `check` counts the files of the commit in progress
  as tracked, and a command that would create a file under `git commit -a`
  or `git commit <paths>` is refused, since its intent-to-add mark would be
  lost.
- A starting directory inside the git directory, and a `GIT_COMMON_DIR`
  other than the repository's own, are refused with exit 4.
- `-C` refuses a path that is not a directory, as git does, instead of
  running from the directory that contains the file.
- `explain` shows under each subject the errors of its project that stop it
  being judged, such as an unknown `--context`, and exits 3 for them as
  `check` does, instead of exiting 0 with "not judged".
- Replacement refs made with `git replace` are not applied anywhere: the
  commit list of the review screen reads commits as stored, as the clocks
  always did, so every clone gives the same result.

### Fixed

- A relative source pattern now matches files in its document's own
  directory even when that directory's name contains `*`, `?`, `[`, `]`, `{`
  or `}`. Before, the name was read as glob syntax, so the pattern matched
  other directories or was rejected as invalid.
- A `.lock` file is no longer taken for an owner lockfile merely because its
  directory's path ends in `.source-sentry/remotes`, as in
  `archive/not.source-sentry/remotes/data.lock`. It can be a source and is
  matched by patterns.
- A `.source-sentry/remotes/<owner>.lock` file in a directory that is not a
  project root is no longer read as an owner lockfile of the enclosing
  project.
- A nested project's check now reports an enclosing lockfile that cannot be
  read, instead of passing without knowing whether declarations were
  stranded there.
- Diagnostics about a whole declaration, such as `missing-document` and
  `double-claim`, now give the line where the declaration begins.
- A preset that defines a context with an empty `facts` list is now an
  `invalid-context` configuration error even when no config imports it.
- A multi-line quoted or flow value in a document's frontmatter keeps track
  of its own lines when a later value is left unclosed, so a `source-sentry:`
  line inside a multi-line title is no longer mistaken for the declaration
  and rewritten.
- `rename` no longer lets you move the only entry of a `living` series out
  of the series, which left the series with no head while `check` still
  passed. It refuses, as `retire` does, and asks for a successor or for the
  series to be removed from its config first.
- `check` is fast again for series and documents on a cadence that started
  long ago, or that has a long backlog of missed windows. It no longer
  recomputes the cadence from its start for every search, or compares every
  entry and window with every other.
- In a nested project's JSON report, a source pattern outside the project is
  written `../src/*.txt` instead of `/../src/*.txt`, and a local path spelled
  like a remote identity, such as `abstract:guide`, is made relative to the
  project.
- Commands that messages suggest now run when pasted into the shell you
  started in. `rename`'s `git mv`, the `git add --renormalize` advice for
  CRLF line endings and the `track --from` advice of the on-file `seal`
  refusal honour `-C`, name their files after `--`, and carry every
  required argument.
- `diff --format json` writes an `attest` array that selects exactly the
  source it showed when run from the document's project root. Before, the
  array could be refused as ambiguous, and in a nested project the command
  could fail outright. `diff --help` now names that directory correctly.
- A file saved by another program while a write is being applied now fails
  the write and is kept, instead of being overwritten, even when only its
  line endings or the text inside `$Id$` changed. The same holds for a file
  saved after the command planned its changes, for example while it waited
  for confirmation. A short window right before the replacement remains.
- A write that fails part-way no longer undoes a file that another program
  saved after the write had changed it. The file is kept, and the failure
  names it.
- A directory above a file that becomes a symbolic link while a write is
  applied now fails the write. On Unix, a permission change made during a
  write is kept.
- A regular file that replaces a tracked symbolic link is now hashed as git
  would add it. Where git checks symbolic links out as placeholder files
  (`core.symlinks=false`, the Git for Windows default), an edited
  placeholder is hashed as `git add` stores it: line endings are converted
  under `text`, `working-tree-encoding` and `ident` are applied, and a clean
  filter is refused as for any file. An unedited placeholder keeps the oid
  the index holds. An edited placeholder marked `assume-unchanged`, which
  git does not compare, stops `check` with exit 4 and the command that
  clears the mark.
- A source in a merge conflict is classified by the file on disk. A regular
  file beside a link or submodule stage is hashed as `git add` stores it
  and is no longer reported missing, and CRLF kept in the index is
  reported exactly when `git add` would keep it.
- `check` now evaluates only the cited document of another project and what
  it depends on. It reports that project's configuration errors and the
  errors met while hashing the cited document's sources, which were
  dropped before, and a fault in an unrelated document of that project no
  longer stops the check.
- A document that cites a renamed document, in its own project or another,
  now keeps its drift clock, deadline and facts across the rename, instead
  of losing the date or moving it later. It keeps them after the renamed
  document is retired, too.
- A document that is overdue because a change could not be dated no longer
  calls the future deadline of its other sources missed. Its line reads
  `overdue, timing unknown` until that deadline has passed.
- Config patterns such as `documents.ignore` now read braces literally, as
  `.gitignore` does. `{a,b}` alternation is for source patterns only.
- Character classes in config patterns read escapes as `.gitignore` does:
  `[\]{]x.md` ignores `]x.md` and `{x.md`, and `\]` no longer closes a
  class.
- Config patterns read named classes such as `[[:alpha:]]`, and a hyphen
  after a range as in `[a-c-e]`, as `.gitignore` does. A class name git
  does not know, such as `[[:word:]]`, is a configuration error.
- A config pattern that ends in an escaped space followed by unescaped
  spaces keeps the escaped space, as `.gitignore` does, instead of failing
  with "dangling '\'".
- In a nested project, an unknown `--context` is reported against that
  project's own config, not the enclosing repository's, and messages about
  contexts and settings name "the project's config".
- Messages call `.source-sentry/source-sentry.yaml` a "project config",
  where they said "repository config", and name the scope of a setting or
  a series "the project", where they said "the repository". The recipes
  and their guide use the same words. The JSON value `repository` is
  unchanged.
- An invalid context now stops every command that works in its project
  with exit 3, as `check` reports it. Before, `track`, `attest` and the
  other commands that write went on, and so did `explain` and `diff`.
- A declaration that holds one pattern under two spellings, such as
  `src/*.rs` and `/docs/src/*.rs` in `docs/guide.md`, is refused as one
  holding two spellings of one path is, and `track` refuses to add such a
  second spelling. Before, a command that named the second spelling
  changed the first.
- In a nested project, sources declared in a document's frontmatter are
  checked for repeats from the project's root, as lockfile entries are.
  Before, `/src/a.rs` and `:/src/a.rs` were refused as one file, and two
  spellings of one file were accepted.
- Moving a frontmatter declaration to a lockfile, or removing one of its
  sources, no longer deletes the comments between it and the next key.
- `rename`, series successors and `--pattern` keep a relative pattern in a
  directory whose name contains glob characters, such as `docs/[draft]`.
  Only a pattern that would have to spell such a directory is refused.
- An empty `source-sentry.reviewer` or `user.name` in git's configuration
  now hides a name set at a broader level, as git itself reads it. Before,
  the broader name signed the review.
- `untrack` says "1 source remains" when one source is left.
- Attesting one source of a living series entry no longer completes the
  entry while a declared pattern that matches nothing has never been
  reviewed.
- A declaration that defines a YAML anchor is no longer rewritten when
  another frontmatter key is malformed. The write is refused instead of
  leaving a dangling alias.
- Preset imports may nest at most 32 deep and expand to at most 4,096
  presets per config. A longer chain is a configuration error instead of a
  crash, and a multiplying chain no longer exhausts memory.
- A command whose output cannot be written, such as a JSON report piped to
  a reader that has exited, now exits 4 and says why on standard error,
  instead of exiting with its ordinary status.
- The `dependency-cycle` diagnostic says when other cycles may remain among
  the same documents.
- `check` and `explain` no longer print the same `outside-authority` advice
  twice for one declaration. The summary still counts each failure.
- The install guide says that the moon `check` task covers the project at
  the workspace root, and that nested projects are checked with
  `source-sentry -C <project> check`.
- A file whose filter driver the attribute files cannot rule out is now
  refused as an unsupported filter, so the driver never runs. This holds
  when the global, system or `info/attributes` file is a symbolic link,
  which git follows, and when an attributes file cannot be read.
- Files whose names are close to the file system's 255-character limit can
  now be written and deleted.
- Reading history reports an unreadable tree as an error instead of treating
  the directory as absent, and an author name containing control characters
  no longer spills into the commit subject.
- The full-screen review records only what it showed:
  - a review of picked sources is refused when one of them changed on disk
    after its changes were shown, and the new changes are shown;
  - a document is read again when its text, or a stored review of one of its
    sources, changed outside the screen;
  - dragging onto another document's row withdraws an answer held with `y`
    or `r`, instead of recording it for that document on Enter;
  - leaving the header shows the document the cursor returns to, so the next
    answer is recorded for the highlighted document.
- In a full-screen review started for named documents, a picked document
  from the other documents that the confirmation listed is now recorded,
  instead of the batch reporting "Recorded 0 reviews."
- When one write of an answer given to several documents fails, the
  full-screen review says how many reviews were recorded and which document
  was not, and prints its closing before it ends.
- In the full-screen review, a reviewer name typed in the header is checked
  before it is remembered, so an email address is refused there and not at
  the next review.
- In the full-screen review, the mouse wheel over the sidebar ends a range of
  picked rows, and a drag made with Shift or Control held is left to the
  terminal, as such a click is.
- When the full-screen review finds nothing to review, its closing follows a
  final evaluation, so a change made meanwhile is reported.
- The full-screen review no longer cuts off the end of rows of wide
  characters, such as CJK text, and a line too long to fit in the pane can
  be scrolled to its end.
- In the full-screen review, the facts at the top of the pane no longer draw
  over the sidebar, and very long facts no longer crash the screen; they are
  cut with an ellipsis.
- In the full-screen review, the confirmation of `a` or of picked documents,
  a path or name shown whole, and the projects box scroll when they hold
  more than fits, so every document, character and project can be read. The
  arrow keys, Page Up, Page Down and the wheel scroll these boxes instead of
  closing them.
- `install.sh` quotes the install directory in the `fish_add_path` line it
  prints, so a directory containing a space is passed as one argument.
- The recipes' guide protects the installed Forgejo workflow file, every
  `.source-sentry/` directory including those of nested projects, and
  GitLab's own CODEOWNERS file in its CODEOWNERS examples.
- `migrate` refuses a schema 1 lockfile that is invalid, with exit 3,
  instead of reporting it current.
- `check` reports a declaration that cannot be read on a document another
  project cites, instead of treating that document as an untracked source.
- `check` reports an unreadable lockfile in another project when it lies in
  the directory of a document it cites, or in one above it, since that
  lockfile could hold the document's declaration. Unreadable lockfiles
  elsewhere in that project are still left to that project's own check.
- `check` reports a duplicate version in another project's series, or
  another diagnostic of that project, whichever of the documents it names is
  cited, not only when the first in path order is.
- The human report of `check` names the deadline of a due source document in
  another project, instead of saying that it "waits on a document that is
  due".
- Taking back an answer given to several documents no longer erases a change
  that another program made to their lockfile while the answer was being
  written. The answer is kept, and the refusal says that another program
  changed the file and names it.
- `review` no longer leaves the terminal on the alternate screen in raw mode
  when it cannot take the terminal over whole, for example when the mouse
  cannot be read.
- `review` expands a tab that follows a wide character, such as an
  ideograph, or a combining mark to the right tab stop.
- The confirmations of `review` count a document's comparison as unseen when
  its reviewed version changed outside the session since it was shown.
- `GIT_ICASE_PATHSPECS` and git's other global pathspec settings no longer
  make Source Sentry's git commands fail.
- `review -C <path>` opens the document in the editor by its full path, so
  the editor gets the right file, and reads the editor and pager settings
  from the repository in use.
- The advice to clear an assume-unchanged mark, and to renormalise a file
  whose index holds CRLF, repeats `-C` and names the file from where the
  command was run.
- A remote document's clock no longer reads history older than it needs, so
  a partial clone (`--filter=tree:0`) or an object database missing an old
  commit or tree no longer stops `check` with exit 4 when recent commits
  settle the clock, and checks of remote documents no longer read every
  commit back to the root.
- Moving a remote document to an owner in another project (`track --from
  remote:a --to remote:b@other`) no longer restarts its clocks.
- In the line mode of `review`, a source changed just after a review was
  recorded, before the session evaluated again, no longer passes unasked
  when the document still waits on an upstream document; the document is
  shown again.
- In the full-screen review, a document reviewed whole that still waits on
  an upstream document moves to the other documents, as the guide says,
  instead of staying in the queue to be reviewed again. It returns when its
  sources or that upstream document change.
- An `invalid-config` or `invalid-schedule` diagnostic no longer repeats
  the config's path and position at the start of its message, after the
  location that names them. Run from a nested project, the repeated path
  was relative to the repository rather than to the current directory, so
  it did not lead to the file.

## [0.1.0] - 2026-11-14

### Added

- The README walks through Source Sentry in five minutes, with real output,
  on a public example repository.

### Changed

- 0.1.0 is the first release. Source Sentry itself behaves as in
  0.1.0-rc.3; the other changes are to its documentation.
- The GitHub Actions and Forgejo Actions recipes have run on real CI. The
  GitLab CI and Woodpecker recipes are released untested, and their headers
  say so.
- The install guide's commands and `cargo install` no longer name a version:
  they install the latest release.

### Fixed

- The Forgejo Actions recipe is copied to `.forgejo/workflows/source-sentry.yml`.
  Its guide named `source-sentry.yaml`, which Source Sentry reads as a
  directory config, so every gate stopped with exit 3.

## [0.1.0-rc.3] - 2026-11-14

### Added

- `source-sentry completions <shell>` prints shell completions for bash, zsh,
  fish, PowerShell and elvish.
- Install scripts for Linux, macOS and Windows, in `install/`, that verify
  the release archive against `SHA256SUMS` before installing the binary, and
  never change `PATH` or shell configuration.
- `cargo binstall --registry gmn source-sentry` installs the release binary
  instead of compiling the crate.
- A proto plugin that installs a version pinned per repository, and a moon
  project whose `check` and `review` tasks install Source Sentry through it.
- Each release is also published, with the same files, on the GitHub
  mirror's releases page.

## [0.1.0-rc.2] - 2026-11-14

### Added

- The first release candidate of Source Sentry: the `source-sentry` command
  and the `source-sentry-core` and `source-sentry-git` libraries.
- Due dates. A stale document is `due` from the first instant of its review
  day, set by its grace period and cadence, and `upstream-due` when a
  document it uses as a source is due. The built-in default context gives a
  stale document a notice until it is due, and warns from then on, so a
  cadence gathers the changes of many commits into one review. A document
  without a schedule is due as soon as it is stale.
- An uncommitted change is judged as if committed at the instant of the
  check to decide whether its document is due, and its deadline is reported
  as provisional.
- `review` lets documents that are not due yet wait for their review day:
  the full screen lists them with a `not due` tag, the line mode asks about
  them only with `--all`, and `a` includes them only on request.
- CI recipes for GitHub Actions, GitLab CI, Forgejo Actions and Woodpecker,
  whose scheduled runs notify on a document's review day.
- An Agent Skill that teaches coding agents to keep documentation current
  with Source Sentry's commands, including when to give a document a
  cadence.
