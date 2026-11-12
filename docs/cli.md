# Command-line interface

```
source-sentry [--repo <path>] [--color <auto|always|never>] [--quiet] <command>
source-sentry help [<command>]
```

- `--repo` names the repository; by default, the current directory, searching
  upward as git does. Relative repository locations start in the current
  directory; a leading `/` starts at the root of the containing repository.
- `--color` defaults to `auto`, which respects `NO_COLOR` and redirected
  output.
- `--quiet` prints only errors and what was asked for: the JSON of
  `--format json` and, on a terminal, a change's plan with its question.
- `-h` or `--help` describes a command; `-V` or `--version` prints the
  version.

Documents are named by path or by remote identity: `abstract:onboarding-video`,
`google-drive:1a2B3c4D5e6F7g8H9i0J`. Every command-line path is relative to the
current directory, as in git; a leading `/` means the repository root. This
also applies with `--repo`: it selects the repository without changing the
current directory. Native drive-qualified paths are accepted on Windows.

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
is reported but never changes the exit code.

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
  command as a list of arguments. The array uses `/`-rooted document,
  path-source and expectation arguments, so repository paths do not depend
  on the current directory; declared pattern selectors keep their spelling.
  The report's other path fields retain their repository-relative form
  without a leading `/`.

`--reviewer` defaults to git's `user.name`. `--reread` also records that the
whole document was reread.

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
  or from the repository root with `/`; the flag alone chooses the nearest
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
- **A placement** is written `on-file`, `lockfile:<directory>` or
  `remote:<owner>`. A lockfile directory is relative to the current
  directory, or `/`-rooted.
- **`init`** registers the documents that a root's scan finds and nothing
  claims. Without paths it only previews roots, claims and candidates. With
  paths, or `--all`, it registers them as unattested. Candidate paths start
  at the current directory, or at the repository root with `/`. It never
  creates a root: `track` a first document, or add a directory config with
  `root: true`.

  Series entries inherit as with `track`, using the declarations present
  when the command starts. Classes and owners given to `init` add to the
  copied declaration. The plan shows each predecessor and what is copied.
- **`migrate`** upgrades lockfiles to the current schema. Schema 1 is the
  only one so far, so it confirms that every lockfile is current, and refuses
  a schema it does not know.

## Confirmation

A command that changes files prints its plan first. On a terminal it then
asks for confirmation; `--yes` skips the question, and `--dry-run` prints the
plan only. Without a terminal and without `--yes`, it refuses, so CI never
changes files silently.

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
