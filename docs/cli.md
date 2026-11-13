# Command-line interface

```
source-sentry [-C <path>] [--color <auto|always|never>] [--quiet] <command>
source-sentry help [<command>]
```

- `-C <path>` runs as if started in that directory, as in git. Relative
  arguments and project selection start there. Relative and absolute
  filesystem paths are accepted. Without it, use the current directory,
  searching upward for the repository as git does.
- `--color` defaults to `auto`, which respects `NO_COLOR` and redirected
  output.
- `--quiet` prints only errors and what was asked for: the JSON of
  `--format json` and, on a terminal, the review session or a change's plan
  with its question.
- `-h` or `--help` describes a command; `-V` or `--version` prints the
  version. On a terminal, the top-level help opens with the Source Sentry
  mark, drawn in half-block characters; redirected help is plain text, and a
  command's own help never carries it.

Documents are named by path or by remote identity: `abstract:onboarding-video`,
`google-drive:1a2B3c4D5e6F7g8H9i0J`.

Document arguments, `init` paths, `--lockfile` directories,
`lockfile:<directory>` placements and `-C` are filesystem paths. Relative
paths start in the current directory, as in git. Absolute paths use the
platform's native form: `/home/me/project/docs/guide.md` on Unix, or
`C:\project\docs\guide.md` on Windows. A leading `/` has its filesystem
meaning. `-C` changes the effective starting directory for every relative
argument.

Source spellings, given to `--source` of `track`, `untrack`, `diff` and
`attest`, to `--pattern` of `track` and `untrack`, and the path of `--expect`,
start in the current directory when relative.
`diff` and `attest` select declared patterns through `--source`.
In source spellings, `/` means the document's
project root and `:/` means the repository root: `/src/a.rs` names
`src/a.rs` inside that project, while `:/src/a.rs` names the repository's
`src/a.rs`. Sources may leave a project, using parents or `:/`, but may never
leave the repository. Remote documents use the project containing their
owner lockfile. Their declarations require rooted sources; relative
command-line input is resolved from the effective starting directory and
stored rooted.

Human output displays paths relative to the effective starting directory,
including diagnostic locations and commands printed for copying. Paths are
absolute when the current directory lies outside the working tree. Quoted
source declarations retain their spelling. JSON subjects and diagnostic
locations include `project`, the repository path of the project root (`.`
for the repository project), and paths relative to that project. A source
outside its project uses a relative parent path in the report.

A directory containing a tracked `.source-sentry/source-sentry.yaml` starts
a project. The repository root is always a project. The nearest project
owns a path, and enclosing configs do not apply inside a nested project.
Document commands select the document's project; remote identities select
the project containing the effective starting directory. See
[`structure/README.md`](../structure/README.md#projects) for configuration
sharing through explicit preset imports.

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

`check` evaluates the project containing the effective starting directory,
using its own configs and contexts. Nested projects are excluded, and human
output names them in a note. Run `source-sentry -C <project> check` for each.
A source document in another project contributes its freshness under that
project's own declarations and schedules, without applying its contexts.

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
`inherit-from` with that predecessor's project-relative path.

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
                     [--plain]
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
  A previously reviewed pattern keeps its review when its last member's
  removal is acknowledged. It then has only the `empty-pattern` warning,
  without becoming `unattested` or regaining its declaration deadline.
  It can be attested again, including with `--reread`.
- **`--source`** selects a path source by the file it resolves to from the
  current directory, from the document's project root with `/`, or from the
  repository root with `:/`. A pattern is selected by its exact declared
  spelling or its `/`- or `:/`-rooted form. A value
  that selects both a path and a pattern is refused, naming both.
- **`--expect`** takes a path from the current directory, or a `/`- or
  `:/`-rooted path, followed by `@oid` or `@absent`. Parent components are accepted;
  the last `@` separates the path from its state.
- **`--format json`** prints one object: its `schema`, the `document` and
  `revision`, each of the `sources` with every file's `baseline`,
  `current`, `changed` and `diff`, the `expectations`, and the `attest`
  command as a list of arguments to run from the document's project root,
  named by `project`. The document uses
  its project-relative path; source and expectation arguments use rooted
  spellings, with `:/` for repository paths outside a nested project.
  Declared pattern selectors keep their spelling. The array omits `-C`
  because its starting directory is explicit. Other paths are relative to
  the document's project.
  Each source also carries its own `reviewed` (`at` and `by`) when present;
  a pattern's review is independent of its files' baselines.

`--reviewer` defaults to `source-sentry.reviewer` in git's configuration when
it is set, and otherwise to git's `user.name`. Set it with
`git config source-sentry.reviewer "<name>"`, at any level git offers, or
from the header of the full-screen review. `--reread` also records that the
whole document was reread.

Attesting a selected pattern records its own `reviewed` and the same review
on every current member, and removes former members. Selection covers whole
sources; individual pattern members cannot be selected. Attesting a path
that also matches a pattern leaves the pattern and its members unchanged.
Unselected sources keep their reviews. A pattern's review counts as evidence
that the document was reviewed, but does not attest new matches or record a
whole-document reread. Member oids still determine source changes, and
stored review timestamps never date deadlines.

`review` traverses the starting project and every project nested inside it,
using each project's configs. It walks the live documents whose facts
include `stale`, `overdue`,
`unattested` or `upstream-stale`. `--context` defaults to `default` and
decides their order. A project lacking that context uses its own `default`,
and the session announces the fallback when it starts. The queue puts the
worst outcome first, then upstream documents before their dependents within
an outcome, then path. Named documents limit the
queue to those that need review. Sealed records are never queued. A
configuration or integrity error refuses the session before its first
question.

### The full screen

Where the terminal shows 24-bit colour and has at least 80 columns and 20
rows, `review` takes over the screen. Elsewhere, and with `--plain`,
`--color never` or `NO_COLOR`, it runs in the line mode below. Windows
consoles are taken to show 24-bit colour; other terminals announce it in
`COLORTERM`. A session with nothing to review prints its closing alone.

The screen has four parts:

- **A header** with the reviewer, the context, the number of projects
  traversed and the session's progress.
- **A sidebar** that lists the documents that need review, in queue order,
  above a collapsed list of every other document: those reviewed in this
  session first, marked with a tick, then the other live documents, then the
  sealed records. The mark fills the space below while that list is closed.
- **A pane** for the document under the cursor: its outcome and facts, when a
  source was last reviewed and by whom, its project and the upstream
  documents it waits on. Below these it shows either the changed files with
  the selected file's changes, or the document's own text. From 200 columns
  it shows both, side by side.
- **The question** about that document, in the last rows, with the keys
  that answer it.

The order is the reviewer's. The queue's order is only where the cursor
starts, and any document may be opened at any time.

| Key | Action |
|---|---|
| `↑` `↓` `Tab` | Move in the list that has the focus; `Shift+Tab` moves back, and `Tab` goes round |
| `→`, `Enter` | From a document, go to its changed files |
| `←`, `Esc` | From the changed files, return to the documents |
| `Enter` | On a changed file, show its whole path; on the label of the other documents, open or close them |
| `o` | Open or close the other documents |
| `↑` | From the first document, enter the header |
| `v` | Swap the pane between the changes and the document |
| `PgUp` `PgDn` | Scroll the pane by a page |
| Mouse wheel | Scroll the pane under the pointer by three rows; over the sidebar, move the cursor |
| `y` | Answer that the document is still accurate |
| `r` | Give the same answer and record a whole-document reread |
| `Enter` | Record a held answer; `Esc` or any other key withdraws it |
| `a` | Answer `y` for every document that needs review, after a confirmation |
| `e` | Edit the document in an editor, then evaluate again |
| `?` | Show the keys |
| `q`, `Ctrl+C` | Quit, preserving earlier reviews |

The arrow keys and Tab move in the list that has the focus. It starts in the
sidebar, where the row under the cursor is marked in a strong blue. From a
document, `→` or Enter moves the focus to its changed files: the document's
row turns a muted blue, selected but no longer focused, and the selected
file takes the strong one. There, Enter shows the file's whole path, which a
row may have cut. `←` or `Esc` returns the focus to the sidebar. The label
of the other documents is a row of the sidebar too, between the queue and
those documents.

`y` and `r` only hold an answer: nothing is written until Enter. A recorded
review is bound to the source versions shown, exactly as in the line mode,
and a source changed since refuses the write and shows the document again.
The reviewed document then moves to the other documents, and the cursor to
the next one that needs review. A document that needs nothing takes `r` but
no `y`; a sealed record can only be read.

Each document is compared once per evaluation, against the working tree as
that evaluation read it, and the documents after the cursor are read while
the reviewer is idle, so that moving between documents does not wait. A
source edited since is caught when the review is recorded: the write reads
every source again and refuses what changed. The working tree is evaluated
again after every write, every editor visit and every change of context.

Because the screen reads the mouse for its wheel, selecting text with the
mouse needs the terminal's own modifier, which is Shift in most terminals.

`a` lists every document that needs review with its outcome and whether its
current comparison has been displayed, and records them all on `y`. Any
other key goes back. A source that changes during the batch stops it.

In the header, `←` and `→` select a field and Enter changes it. The
reviewer's name applies to the reviews recorded from then on, and is
remembered for the repository as `source-sentry.reviewer` in its local git
configuration; an empty name forgets it. The context
is chosen from those that the traversed projects declare, and the working
tree is evaluated again under it. The projects are listed. `↓` returns to
the documents.

The screen is drawn on the terminal's alternate screen and leaves nothing
behind, so the closing described below is printed after it. `e` hands the
terminal to the editor chain of the line mode, and takes it back when the
editor closes.

### The line mode

A session whose queue is not empty opens with the Source Sentry mark and,
beside it, the version, the licence and the facts of the review ahead: the
reviewer whose name each review will carry, the context, the number of
projects traversed, how many documents need review out of those tracked (out
of those named, when documents were named), and the queue's outcomes, worst
first. `--quiet` leaves the opening out.

Each document shows its place in the queue under a rule, then its name with
its outcome and facts. Each pending file follows with its line counts and
the first seven digits of its old and new object names; the lockfile holds
the full names. Up to 40 diff lines are shown across the document, in
groups. Each group opens with a `path:line` location relative to the current
directory, and each line carries its number, with tabs expanded to stops of
four columns. The display is as wide as the terminal, between 60 and 100
columns.

The session then asks whether the document is still accurate. Type a
one-letter answer and submit it with Enter:

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
it tries `GIT_PAGER`, `PAGER` and git's `core.pager`, then plain output. The
full diff is the same display without the line limit; opened in an editor or
a pager, it is a file without colour, 100 columns wide. `e`
uses the editor chain alone and supports local documents. Commands may
include arguments. A remote document must be edited in its source
application.

### Both modes

`review` requires both standard input and standard output to be terminals.
Otherwise it exits 1 and points to `diff` and `attest`. There is no `--yes`.
End of input or cancellation quits. A completed or quit session exits 0;
the closing lists what was recorded and what still needs review. The line
mode evaluates the working tree once more for it; the full screen reports
its latest evaluation. It mirrors the opening: the mark with the reviewer, the number
of reviews recorded, the number of documents remaining and the session's
length beside it, then each recorded review and each remaining document. Use
`check` as the gate. Review comparisons and questions remain visible under
`--quiet`, which reduces the closing to plain sentences without the mark.

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

`--by` has the same default as `--reviewer`.

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
  `reviewed`, its seal, `completed` and `renamed` are never copied. Options
  add sources, classes and owners, and override grace or cadence, as when
  extending a tracked document. Placement follows the usual rules. The plan names the
  predecessor and shows the copied declaration before the options apply.
  A source that cannot be re-expressed or registered refuses the whole plan.
- **`--source` and `--pattern`** name files and globs from the current
  directory, from the document's project root with `/`, or from the
  repository root with `:/`. A relative input is
  stored relative to the document's directory: from the root,
  `track docs/cli.md --source crates/cli/src/cli.rs` writes
  `../crates/cli/src/cli.rs`. A `/`-rooted input is stored as given.
  A pattern is rebased by the literal components before its first glob
  component. Invalid glob syntax, `..` after a glob component, and directory
  names containing glob metacharacters are refused with a usage error
  explaining why they cannot be rebased safely. A remote document's sources
  are stored with `/` or `:/` because it has no directory.
- **`--lockfile <directory>`** names a directory from the current directory,
  or by an absolute filesystem path; the flag alone chooses the nearest
  lockfile.
- **`--cadence`** requires a valid date and recurrence rule. A malformed date
  is invalid usage (exit 64); a parsed cadence that violates the schedule
  rules is refused (exit 2). Invalid cadences already written in configs or
  declarations remain configuration errors (exit 3).
- **`--from` and `--to`** move declarations with their reviews and
  every tombstone of each moved key. A move between lockfiles must include
  every entry whose nearest lockfile changes, and a lockfile left empty is
  deleted. Only a document read as `md-utf8` moves on-file, and never a record
  or a source of another document. The plan shows any settings that change
  with the move, such as an owner's defaults.
- **`untrack`** removes the declared sources it names. A path is matched by
  the file it resolves to from the current directory, `/` or `:/`, and a
  pattern by its declared spelling or `/`- or `:/`-rooted form. The other
  sources keep their reviews. To stop tracking a document altogether, `retire` it;
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
  members are re-expressed for the new directory and project. A `/` source
  outside the destination project becomes `:/`; `:/` sources retain their
  repository target. The destination must define every class explicitly
  used by the declaration.

  Every source review, the document's `reviewed`, its seal and `completed`
  are kept. The declaration appends `renamed: [{from, at, by}]`, newest last,
  with the current time and the same name `--reviewer` defaults to (or
  `null`). `from` uses
  `/docs/old.md` from the destination project's root, or `:/other/old.md`
  from the repository root when the former path is outside that project.
  Project moves also re-express earlier provenance entries and show each
  rewrite in the plan. Evaluation follows earlier identities before each
  rename's introducing commit, keeping obligation clocks and series credit
  before and after committing the move. Informational timestamps never date
  those clocks. Other documents' path sources follow the new path and retain
  their reviews. A reviewed pattern member follows only when the pattern
  also matches the new path; otherwise the pattern stays unchanged and
  `check` reports the membership change. Conflicting destination member
  reviews are refused. A changed sealed record is refused; other content
  changes are reported against the retained review oids by `check`.

  The plan shows every source rewrite, effective setting change and change
  to series membership or version. A rename that would introduce configuration or integrity errors
  is refused. Use `--dry-run` to preview, or `--yes` to confirm without a
  terminal, as for other changes.
- **A placement** is written `on-file`, `lockfile:<directory>`,
  `remote:<owner>` or `remote:<owner>@<project-directory>`. Directories are
  filesystem paths resolved from the effective starting directory, or
  absolute paths. An unqualified remote owner belongs to the starting
  project. An explicit remote directory must be a tracked project root
  (the repository root counts); a refusal names the nearest project root.
  Owner identifiers cannot contain `@`. Plans show resolved projects.

  A local document's declaration must stay in its own project. To move the
  document between projects, use `git mv` followed by `source-sentry rename`.
  Adding a project marker can strand declarations in an outer lockfile;
  `check` names the repair command. Move those declarations with
  `--from lockfile:<outer> --to lockfile:<inner>`, or `--to on-file`.
  The plan re-expresses sources for the new project and shows every changed
  setting; the destination must define the declaration's explicit classes.
  Remote declarations can move between projects' owner lockfiles with the
  qualified placement spelling and the same preview and class checks.
- **`init`** scans the starting project and registers the documents that a
  root's scan finds and nothing
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
