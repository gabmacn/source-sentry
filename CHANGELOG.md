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
  the write and is kept, instead of being overwritten. A short window right
  before the replacement remains.
- A regular file that replaces a tracked symbolic link is now hashed as git
  would add it. Where git checks symbolic links out as placeholder files
  (`core.symlinks=false`, the Git for Windows default), an edited
  placeholder is hashed as `git add` stores it: line endings are converted
  under `text`, and a clean filter is refused as for any file. An unedited
  placeholder keeps the oid the index holds.
- `check` now evaluates only the cited document of another project and what
  it depends on. It reports that project's configuration errors and the
  errors met while hashing the cited document's sources, which were
  dropped before, and a fault in an unrelated document of that project no
  longer stops the check.
- A document that cites a renamed document in its own project now keeps its
  drift clock, deadline and facts across the rename, instead of losing the
  date or moving it later.
- Config patterns such as `documents.ignore` now read braces literally, as
  `.gitignore` does. `{a,b}` alternation is for source patterns only.
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
  refused as an unsupported filter, so the driver never runs.
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
- `install.sh` quotes the install directory in the `fish_add_path` line it
  prints, so a directory containing a space is passed as one argument.
- The recipes' guide protects the installed Forgejo workflow file, every
  `.source-sentry/` directory including those of nested projects, and
  GitLab's own CODEOWNERS file in its CODEOWNERS examples.

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
