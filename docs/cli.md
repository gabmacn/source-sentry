# Command-line interface

```
source-sentry [--repo <path>] [--color <auto|always|never>] [--quiet] <command>
source-sentry help [<command>]
```

- `--repo` names the repository; by default, the current directory, searching
  upward as git does. Relative repository locations start in the current
  directory; absolute filesystem paths are accepted.
- `--color` defaults to `auto`, which respects `NO_COLOR` and redirected
  output.
- `--quiet` prints only errors and what was asked for: the JSON of
  `--format json` and, on a terminal, the review session or a change's plan
  with its question.
- `-h` or `--help` describes a command; `-V` or `--version` prints the
  version.

Documents are named by path or by remote identity: `abstract:onboarding-video`,
`google-drive:1a2B3c4D5e6F7g8H9i0J`.

Document arguments, `init` paths, `--lockfile` directories,
`lockfile:<directory>` placements and `--repo` are filesystem paths. Relative
paths start in the current directory, as in git. Absolute paths use the
platform's native form: `/home/me/project/docs/guide.md` on Unix, or
`C:\project\docs\guide.md` on Windows. A leading `/` has its filesystem
meaning. `--repo` selects the repository without changing the current
directory.

Source spellings, given to `--source` or `--pattern` of `track`, `untrack`,
`diff` and `attest`, and the path of `--expect`, start in the current
directory when relative. A leading `/` means the repository root, as in
source declarations: `/src/a.rs` names `src/a.rs` in the selected repository.

Human output displays repository paths relative to the current directory,
including diagnostic locations and commands printed for copying. Quoted
source declarations retain their declared spelling. JSON keeps paths
relative to the repository root.

## Checking

```
source-sentry check [--context <name>] [--fail-on <warn|fail>]
                    [--rev <revision>] [--at <instant>] [--no-cache]
                    [--format <human|json>]
```

| Option | Meaning | Default |
|---|---|---|
| `--context` | The context that decides outcomes | `default` |
| `--fail-on` | The threshold that makes the exit non-zero | `warn` |
| `--rev` | Check a committed snapshot instead of the working tree | the working tree |
| `--at` | The evaluation instant, in RFC 3339 | now |
| `--no-cache` | Reserved: no cache exists yet, so it changes nothing | |
| `--format` | `human`, or `json` as in `structure/report.example.json` | `human` |

The exit code is:

| Code | When |
|---|---|
| 0 | Nothing reaches the threshold |
| 1 | A subject's outcome is `warn`, and the threshold is `warn` |
| 2 | A subject's outcome is `fail` |
| 3 | A configuration or integrity error |
| 4 | The evaluation could not complete |
| 64 | Invalid command-line usage |

A diagnostic of severity `warning`, such as a pattern that matches nothing,
is reported but never changes the exit code. Exit 0 can still accompany
diagnostic warnings or outcomes below the selected threshold; read the
report even when the gate passes.

Every written cadence is validated when its config or declaration loads,
including unused presets, overridden settings and sealed records. An invalid
cadence is reported once at its written line as `invalid-schedule`, a
configuration error (exit 3). Deadlines and series windows are also checked
when the inherited grace, repository time zone and relevant clocks are
resolved; a valid cadence can still lack a required future due date.

The report gate and the continue gate are two invocations, which differ only
in `--fail-on`. Both receive the same `--at`, captured once by the CI recipe.

Human output, sketched:

```
Context production · 2026-11-07 18:00 UTC · working tree on 4f3c2a1

fail  docs/privacy.md              stale, overdue (due 2026-11-03)
      db/schema.sql changed in 7d2e9b0, counted from 2026-10-20
      production: privacy + stale → fail (.source-sentry/source-sentry.yaml:24)
warn  docs/onboarding.md           unattested since 2026-10-30
note  audits: security-audits      missed the window ending 2026-09-01

warning  empty-pattern  docs/source-sentry.lock:31  `/db/legacy/*.sql` matches no tracked file

1 failing · 1 warning · 1 notice · 9 passing · exit 2
```

A change is dated by its clock, which counts from the earliest committer
date from that commit to the one checked. A later commit dated earlier, for
example by a skewed clock, therefore moves the date before the change
commit's own.

- **An uncommitted change** has no clock yet, so its deadline starts once it
  is committed. Uncommitted edits never reset a clock that is already
  running.
- **An attested version missing from the first-parent history** cannot be
  dated: the document is `timing-unknown`, and also stale and overdue.
- **A clock more than 24 hours after `--at`** is a `future-dated` error,
  exit 4, rather than being moved to the evaluation instant.
- **A shallow clone** stops with exit 4 only when a clock needs history the
  clone does not have.

A diagnostic about a document, such as a missing source, is shown under that
document, which is then listed even when it passes. Other diagnostics follow
the subjects.

For an unclaimed series entry with a declared predecessor, `check` and
`explain` show the `source-sentry track <path>` command and the predecessor
whose declaration it would inherit. The JSON unclaimed subject adds
`inherit-from` with that predecessor's repository path.

`check` reads the files that git tracks: a new document or config counts
once it is added with `git add`, even if it is not committed. On the working
tree, a note names the files that would count but that git does not track
yet. It never changes the exit code.

`explain <document|path|series>` prints each subject it names in full: its
effective settings and where each comes from, its facts, the rules that
matched, the timing evidence and its next deadline. A document claimed twice
is explained once per claim, and a series name defined in several scopes
once per scope. It takes the same `--context`, `--rev` and `--at` as
`check`, but it is not a gate: a failing subject still exits 0. The
diagnostics about the subject follow, and an error among them sets the exit
code as it would for `check`: 3, or 4 when, for example, a clock cannot be
dated. A name that matches nothing exits 2.

## Reviewing

```
source-sentry diff <document> [--source <path-or-pattern>]... [--format <human|json>]
source-sentry review [<document>...] [--context <name>] [--reviewer <name>]
source-sentry attest <document> [--source <path-or-pattern>]...
                     [--expect <path@oid|path@absent>]... [--reviewer <name>]
                     [--reread] [--dry-run] [--yes]
```

`diff` shows each source's changes since its attestation, and ends with the
`attest` command line that binds exactly what was shown, expectations
included. It is quoted so that POSIX shells and PowerShell read it alike;
only a path that holds both an apostrophe and one of `` $ ` \ ! `` or a
double quotation mark is quoted for POSIX shells alone. `diff` exits 0
whether or not anything changed.

- **When the attested version is no longer in the repository,** the whole
  current file is shown against an empty baseline, and `diff` says so. A
  file that is not UTF-8 text has no line diff. A working-tree file whose
  canonical text cannot be rebuilt is shown once it is staged.
- **A document without sources** is reviewed by rereading it, so its command
  carries `--reread`. A pattern that never matched a file and was never
  attested cannot be attested: correct it, or `untrack` it.
- **`--source`** selects a path source by the file it resolves to from the
  current directory, or from the repository root with `/`. A pattern is
  selected by its exact declared spelling or its `/`-rooted form. A value
  that selects both a path and a pattern is refused, naming both.
- **`--expect`** takes a path from the current directory, or a `/`-rooted
  path, followed by `@oid` or `@absent`. Parent components are accepted;
  the last `@` separates the path from its state.
- **`--format json`** prints one object: its `schema`, the `document` and
  `revision`, each of the `sources` with every file's `baseline`,
  `current`, `changed` and `diff`, the `expectations`, and the `attest`
  command as a list of arguments to run from the repository root. The
  document uses its repository path without a leading `/`; path-source and
  expectation arguments are `/`-rooted, and declared pattern selectors keep
  their spelling. Repository selection is implicit in that directory, so
  the array omits `--repo`.
  The report's other path fields retain their repository-relative form
  without a leading `/`.

`--reviewer` defaults to git's `user.name`. `--reread` also records that the
whole document was reread.

`review` walks the live documents whose facts include `stale`, `overdue`,
`unattested` or `upstream-stale`. `--context` defaults to `default` and
decides their order: worst outcome first, then upstream documents before
their dependents within an outcome, then path. Named documents limit the
queue to those that need review. Sealed records are never queued. A
configuration or integrity error refuses the session before its first
question.

Each document shows its place in the queue, the pending sources' old and new
oids, line counts and up to 40 diff lines. Changes carry `path:line`
locations relative to the current directory. Type a one-letter answer and
submit it with Enter:

| Answer | Action |
|---|---|
| `n`, or Enter alone | Leave the document as it is |
| `y` | Record the review, bound to the source oids shown |
| `r` | Record the same review and a whole-document reread |
| `a` | Record all remaining documents after a second confirmation states how many current comparisons have not been shown |
| `d` | Open the complete diff, then return to the prompt |
| `e` | Open the document in an editor, wait for it to close, then evaluate and show it again if it still needs review |
| `q` | Quit, preserving earlier reviews |

For a document without sources, `y` and `r` both record a reread. A source
changed after display refuses that answer and shows the document again with
its current changes. Each accepted review is written immediately through
the same planner and atomic write path as `attest`. The answer confirms
that write; there is no further plan question.

After every write and editor visit, the working tree is evaluated again.
Documents that become fresh leave the queue, and documents that become stale
join it with an explanation of what changed during the session. A skipped
document returns only if one of its sources changes. A reviewed document
returns if a later change requires another review.

Bulk confirmation covers the current remaining queue and its source
versions. Every write still rebuilds the queue: newly stale documents
outside the confirmed batch receive individual prompts, documents that no
longer need review drop out, and changed expectations require another
review. When documents were named, newly stale documents outside that
selection appear in the closing summary.

`d` uses `VISUAL`, then `EDITOR`, then git's `core.editor`; without an editor
it tries `GIT_PAGER`, `PAGER` and git's `core.pager`, then plain output. `e`
uses the editor chain alone and supports local documents. Commands may
include arguments. A remote document must be edited in its source
application.

`review` requires both standard input and standard output to be terminals.
Otherwise it exits 1 and points to `diff` and `attest`. There is no `--yes`.
End of input or cancellation quits. A completed or quit session exits 0;
the closing summary uses a final evaluation to list what was recorded and
what still needs review. Use `check` as the gate. Review comparisons and
questions remain visible under `--quiet`.

## Records

```
source-sentry seal <document> [--by <name>] [--dry-run] [--yes]
source-sentry retire <document> [--by <name>] [--dry-run] [--yes]
```

- **`seal`** makes a document a record: it records the document's oid and
  freezes its sources. A record must never change, and is never resealed.
  Only a lockfile entry can be sealed, so an on-file document moves to a
  lockfile first. Remote documents, and the living head of a series, cannot
  be sealed.
- **`retire`** stops tracking a lockfile entry and leaves a tombstone, which
  keeps a record's seal and a series entry's completion. The file itself is
  not touched. A reused path or remote identity may be tracked and retired
  again: each retirement adds a tombstone to its list, keeping every earlier
  seal and completion. Lists are ordered by retirement instant, oldest
  first; equal instants retain their order. The living head of a series is
  retired only with its series.

`--by` defaults to git's `user.name`.

## Tracking

```
source-sentry track <document> [--on-file | --lockfile [<directory>] | --remote <owner>]
                    [--url <url>] [--source <path>]... [--pattern <glob>]...
                    [--class <name>]... [--owner <id>]...
                    [--grace <days>] [--cadence <dtstart> <rrule> | --no-cadence]
                    [--dry-run] [--yes]
source-sentry track <document>... --from <placement> --to <placement> [--dry-run] [--yes]
source-sentry untrack <document> [--source <path>]... [--pattern <glob>]... [--dry-run] [--yes]
source-sentry rename <old> <new> [--dry-run] [--yes]
source-sentry init [<path>... | --all] [--class <name>]... [--owner <id>]... [--dry-run] [--yes]
source-sentry migrate [--dry-run] [--yes]
```

- **The default target** of `track` is the nearest lockfile, or a new one in
  the document's directory. A remote identity requires `--remote`.
- **`track` on a tracked document** adds to its declaration where it is:
  - new sources, unattested: a reviewed document is stale until they are
    reviewed, and one never reviewed stays unattested;
  - classes and owners it lacks;
  - the grace, cadence or url given.

  Existing reviews are kept. A source already declared is refused, and so
  are a source inside or naming a submodule and a placement option that
  names another placement.
- **`track` on an unclaimed series entry** starts with the newest earlier
  entry, ordered by parsed version, that still has a declaration. A sealed
  predecessor qualifies; a retired entry has no declaration to copy. Both
  series lifecycles use this rule. Without a declared predecessor,
  registration proceeds as usual.

  It copies the declared classes, owners, grace, cadence and sources.
  Relative source paths and patterns are re-expressed for the new document's
  directory; `/`-rooted spellings stay unchanged. Attestations, the document's
  `reviewed`, its seal and `completed` are never copied. Options add sources,
  classes and owners, and override grace or cadence, as when extending a
  tracked document. Placement follows the usual rules. The plan names the
  predecessor and shows the copied declaration before the options apply.
  A source that cannot be re-expressed or registered refuses the whole plan.
- **`--source` and `--pattern`** name files and globs from the current
  directory, or from the repository root with `/`. A relative input is
  stored relative to the document's directory: from the root,
  `track docs/cli.md --source crates/cli/src/cli.rs` writes
  `../crates/cli/src/cli.rs`. A `/`-rooted input is stored as given.
  A pattern is rebased by the literal components before its first glob
  component. Invalid glob syntax, `..` after a glob component, and directory
  names containing glob metacharacters are refused with a usage error
  explaining why they cannot be rebased safely. A remote document's sources
  are stored `/`-rooted because it has no directory.
- **`--lockfile <directory>`** names a directory from the current directory,
  or by an absolute filesystem path; the flag alone chooses the nearest
  lockfile.
- **`--from` and `--to`** move declarations with their reviews and
  every tombstone of each moved key. A move between lockfiles must include
  every entry whose nearest lockfile changes, and a lockfile left empty is
  deleted. Only a document read as `md-utf8` moves on-file, and never a record
  or a source of another document. The plan shows any settings that change
  with the move, such as an owner's defaults.
- **`untrack`** removes the declared sources it names. A path is matched by
  the file it resolves to from the current directory or `/`, and a pattern
  by its declared spelling or `/`-rooted form. The other sources
  keep their reviews. To stop tracking a document altogether, `retire` it;
  for an on-file document, delete its `source-sentry` key instead.
- **`rename`** updates Source Sentry's records after you move a document:

  ```sh
  git mv docs/old.md docs/new.md
  source-sentry rename docs/old.md docs/new.md
  ```

  Both arguments are filesystem paths from the current directory, or
  absolute paths. The old file must be absent and have a live declaration;
  the new file must exist, be tracked by git (including intent-to-add), and
  have no independent declaration. Remote identities are refused.

  A lockfile entry moves to the nearest lockfile above the new path, or a
  new lockfile in its directory if none exists. Old tombstones stay where
  they are. An on-file declaration travels with the file; its declaration
  must match the old path's committed frontmatter, so commit an on-file
  declaration before moving it. Relative source paths, patterns and reviewed
  members are re-expressed for the new directory; `/`-rooted spellings stay.

  Every source review, the document's `reviewed`, its seal and `completed`
  are kept. Other documents' path sources follow the new path and retain
  their reviews. A reviewed pattern member follows only when the pattern
  also matches the new path; otherwise the pattern stays unchanged and
  `check` reports the membership change. Conflicting destination member
  reviews are refused. A changed sealed record is refused; other content
  changes are reported against the retained review oids by `check`.

  The plan shows source rewrites and changes to series membership or
  version. A rename that would introduce configuration or integrity errors
  is refused. Use `--dry-run` to preview, or `--yes` to confirm without a
  terminal, as for other changes.
- **A placement** is written `on-file`, `lockfile:<directory>` or
  `remote:<owner>`. A lockfile directory is relative to the current
  directory, or an absolute filesystem path.
- **`init`** registers the documents that a root's scan finds and nothing
  claims. Without paths it only previews roots, claims and candidates. With
  paths, or `--all`, it registers them as unattested. Candidate paths start
  at the current directory, or are absolute filesystem paths. It never
  creates a root: `track` a first document, or add a directory config with
  `root: true`.

  Series entries inherit as with `track`, using the declarations present
  when the command starts. Classes and owners given to `init` add to the
  copied declaration. The plan shows each predecessor and what is copied.
- **`migrate`** upgrades lockfiles to the current schema. Schema 1 is the
  only one so far, so it confirms that every lockfile is current, and refuses
  a schema it does not know.

## Confirmation

A command that changes files, except `review`, prints its plan first. On a
terminal it then asks for confirmation; `--yes` skips the question, and
`--dry-run` prints the plan only. Without a terminal and without `--yes`, it
refuses, so CI never changes files silently.

New files are marked intent-to-add in git's index, so the next `check` reads
them; stage their contents with the rest of your change. A change that git
could not record is refused before anything is written: a new file that git
ignores, or a path outside a sparse checkout, behind a symbolic link or
inside a submodule.

These commands exit with:

| Code | When |
|---|---|
| 0 | Applied, nothing to change, or `--dry-run` |
| 1 | Not confirmed: declined, or no terminal and no `--yes` |
| 2 | Refused: the request does not fit the repository |
| 3 | Refused because of a configuration or integrity error |
| 4 | The repository could not be read or written |
| 64 | Invalid command-line usage |

The binary is named `source-sentry` only, because `ss` is already the
socket-statistics command on Linux.
