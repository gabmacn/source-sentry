# How Source Sentry works

This document explains what Source Sentry is, the model behind it, and why
its rules are what they are. Each section states a rule and then the reason
for it. For the commands, see [`cli.md`](cli.md); for the file formats,
[`../structure/`](../structure/README.md); for CI,
[`../recipes/`](../recipes/README.md); for coding agents,
[`../skills/source-sentry/`](../skills/source-sentry/SKILL.md).

## What it is for

Documentation describes code, and code changes. Nothing tells the author of a
change that a page somewhere now describes something that no longer exists.
Source Sentry makes that link explicit. Each document declares the files it
describes, its **sources**. When a source changes, the document is **stale**
until someone reads the change, corrects the document if needed, and records
that review.

It keeps two kinds of writing apart:

- **Documentation** must follow its sources: a README, a privacy policy that
  describes the data a schema retains, a tutorial.
- **Records** must not change once written: an audit, a decision, the
  findings of a test run. A record is protected by a **seal**.

Source Sentry does not read meaning. It cannot tell whether a document is
right. It tells which documents a change may have made wrong, gives each a
deadline, and records that a named reviewer looked at exact versions of the
sources. It protects against accidents, such as a forgotten page or an
edited record. Protection against intent is the job of code review and
branch protection; see [Trust](#trust).

## The model in brief

1. A **declaration** names a document's sources, and optionally its classes,
   owners and schedule.
2. A **review** records, for each source, the exact version the document was
   confirmed against, with when and by whom.
3. `check` compares every source with its reviewed version and derives
   **facts** about each document, such as `stale` or `overdue`.
4. A **context**, chosen by whoever runs `check`, maps facts to **outcomes**:
   `pass`, `notice`, `warn` or `fail`.
5. A **gate** turns the outcomes into an exit code that CI can act on.

Facts describe the repository and depend on nothing else. Outcomes are
policy, and depend on the context. The same stale privacy policy can warn on
a development branch and block a production deploy.

## Reviews

**A review covers sources, one by one.** Each source of a document stores the
version it was reviewed at and its own `reviewed`, with `at` and `by`.
Reviewing one source says nothing about the others.

> **Why.** A review is a claim, and it should claim only what was read. A
> date per source records the confirmation against that source, so a
> reviewer who checked one change does not vouch for another.

**A review is bound to exact versions.** `diff` shows what changed in each
source since its review and prints the `attest` command that records the
review, with one `--expect` per file shown. `attest` refuses if any of them
has changed since.

> **Why.** Exact expectations keep an unread change from slipping into
> someone else's approval. They also let the reviewer be someone who does
> not use git: a developer can send the output of `diff` to a legal or
> marketing colleague, and record their approval against exactly what they
> were shown.

**Without expectations, `attest` records the working tree as it is,** and
says what it recorded.

> **Why.** A developer who reviews their own change reviewed the working
> tree in front of them.

**A review records working-tree content, not a commit.** The source change,
the document's correction and the review can be committed together.

> **Why.** Recording a commit would need two commits for every change: one
> to create the version, and one to say it was reviewed.

**Rereading the whole document is a separate claim.** The document's own
`reviewed` means that all of it was reread, and only `--reread` sets it. A
document without sources can only be reviewed this way.

> **Why.** "I checked the document against this change" and "I read the
> whole document again" are different statements, and a reader of the
> history should be able to tell them apart.

**Tracking a document records no review.** A document is `unattested` while
nothing about it has ever been reviewed. Once any review exists, a source
that was never reviewed makes the document `stale` instead.

> **Why.** Registration is not reading. If tracking a document marked it as
> reviewed, the first check would pass over text that nobody had compared
> with its sources.

**A review names a person, never an address, and takes its time from the
clock.** `by` is a name, from `--reviewer`, or from git's configuration, or
`null` when none is known. A name that contains `@` is refused. `at` is the
moment `attest` ran, in UTC, to the second; there is no option to set it.

> **Why.** Lockfiles are permanent and may be published, so email addresses
> stay out of them, and `null` says "unknown" without borrowing a real name.
> The reviewer is recorded separately from the commit's author because they
> may differ: the developer who enters a legal colleague's approval is not
> the reviewer. Times are kept to the second because, in a large team, a
> day holds many reviews. Backdating is not offered, because a review
> entered today was entered today.

**Names and stored times are information, not evidence.** They do not
authenticate anyone, and they never date a deadline. Deadlines come from the
repository's history; see [Clocks](#clocks-and-deadlines).

## What counts as a change

**A source has changed when its content has, as git would store it.** Git's
own conversions apply, line endings included. A change that is reverted
makes the document fresh again.

> **Why.** Windows and Linux must agree on whether a file changed. Git
> already defines the answer, and Source Sentry asks git rather than
> imitating its conversion rules, which is fragile.

**Only files that git tracks count.** A new file counts once it is added
with `git add`, even before it is committed.

> **Why.** A file that exists on one machine and not in the repository is
> absent from CI. Counting it locally would make the two disagree.

**What cannot be compared is an error, never a guess.**

- A file in **Git LFS** is compared by its pointer, which Source Sentry
  computes itself. It never runs the LFS filter, because that filter writes
  into the repository, and `check` must not.
- A file behind **any other clean filter** cannot be compared. `check` names
  the filter and stops; `--rev` evaluates a commit instead, where the stored
  content is known.
- A **submodule** is not supported, and is reported.
- A file whose line endings in the index contradict its attributes is
  reported with the remedy, `git add --renormalize .`, because hashing and
  adding would otherwise disagree about its content.
- A **symbolic link** is compared as git stores it and is never followed.
  Declare the file it points to as a source of its own.

## Clocks and deadlines

A stale document is not yet late. Each changed source has a **clock**, and
the document's schedule turns the clock into a deadline. Past the deadline
the document is also `overdue`.

**A clock starts at the commit where the source first differed from its
reviewed version.** Source Sentry finds it by walking the history backwards
from the commit being checked.

> **Why.** Dating from history gives the same answer on a laptop and in CI,
> today and next month. A date taken from the machine that runs the check
> would depend on who asks, and when.

**Later changes never restart a clock.** Neither does reviewing another
source, nor rereading the document. Each source has its own clock, and the
document takes the earliest deadline still outstanding.

> **Why.** Otherwise a document could stay late forever without ever being
> overdue, simply because its source keeps being edited.

**History is followed along first parents.** A change made on a branch
counts from the commit that merged it.

> **Why.** What matters is when the change landed on the line being
> checked, not when it was first written elsewhere.

**A new obligation starts no earlier than the commit that introduced it:**
a newly declared source, a file that newly matches a pattern, a newly
tracked document.

**An uncommitted change has no clock yet.** The document is stale at once,
but its deadline starts counting when the change is committed. Uncommitted
edits never reset a clock that is already running.

**A clock's date is the earliest committer date from its commit through to
the commit being checked.**

> **Why.** A commit cannot have landed later than a commit that follows it.
> So a commit carrying a wrong date in the future is corrected by the next
> commit that is dated properly, and the deadline does not move with it.

**A clock up to 24 hours after the evaluation is accepted. One further
ahead is an error** (`future-dated`, exit 4), which the next correctly dated
commit resolves.

> **Why.** Clocks drift, and the machine that runs the check may lag behind
> the one that made the commit. A day of tolerance absorbs that. Beyond a
> day, drift no longer explains it, and silently moving the date would
> postpone a deadline on the word of a wrong clock.

**When the reviewed version is nowhere in the history, the change cannot be
dated.** The document is `timing-unknown`, and also `stale` and `overdue`,
with no grace. This happens, for example, when a source is reviewed and then
changed again before either version is committed.

> **Why.** The only other date available is that of the review, and a
> review can arrive months after the change it reviews. Being late at once
> warns early, and it gives the same answer before and after the commit.

**`check` never fetches.** When a clock needs history that a shallow clone
lacks, it stops with exit 4 and says how to fetch it.

**A document keeps its clocks when it moves.** History is followed through
its earlier tracking places, and `rename` records where a document came
from, so that neither a move nor a rename restarts a clock.

> **Why.** The earlier path is recorded rather than inferred, because
> inference from the lockfile's history can be misled by a hand edit, or by
> two changes in one commit. And when an old declaration cannot be read,
> it may only bring a deadline earlier: uncertainty never grants credit.

All of this assumes that the history can be trusted.

## Schedules

**A document is due on the first cadence date on or after its change's date
plus its grace.** Without a cadence it is due when the grace ends; with no
grace and no cadence it is due at once. Grace is a whole number of calendar
days, and a deadline falls at the end of its day in the project's time zone.

> **Why.** Some documents must follow the code immediately. Others, such as
> a marketing analysis, are produced later and in batches. Grace alone
> scatters deadlines over arbitrary dates; a cadence alone can demand work
> the day after a change. Together they say "on the next review day that is
> at least this far away".

**A cadence is a date-only recurrence rule (RFC 5545) with a start date.**

> **Why.** Cron cannot cleanly say "the first Monday of the month": it
> combines the day of the month and the day of the week with "or".

**The start date must itself be a date of the rule.**

> **Why.** It fixes how intervals are aligned, and makes the first due date
> a real due date.

**Time zones are IANA names, and UTC by default.**

> **Why.** A fixed offset is wrong for half the year wherever clocks change
> for daylight saving.

**Every written schedule is validated, used or not.**

> **Why.** Whether a configuration is valid must not depend on which
> document happens to use a setting.

**A schedule belongs to a document, a directory, a preset or a series,
never to a context.**

> **Why.** When work is due must not depend on who asks.

`cadence: null` removes an inherited cadence without cutting the rest of
the inheritance. The supported rules have limits: no time of day, no count
or end date, no list mixing plain and numbered weekdays, and no occurrence
more than 400 years ahead.

## Facts, contexts and outcomes

| Fact | Meaning |
|---|---|
| `fresh` | Every source is as it was reviewed |
| `stale` | A source changed since its review, or was never reviewed |
| `overdue` | A deadline has passed |
| `unattested` | Nothing about the document was ever reviewed |
| `upstream-stale` | A document it uses as a source is stale or overdue |
| `timing-unknown` | A change could not be dated |
| `unclaimed` | An eligible file in a documentation root that nothing tracks |
| `awaiting-seal` | A superseded entry of a series that is not sealed yet |
| `missed` | A window of a series closed without its work |

**Facts are independent, not stages.** An overdue document is still stale.

> **Why.** If becoming overdue replaced being stale, a document could
> escape a rule written for stale documents at the very moment it became
> more urgent.

**A context maps facts, and optionally classes, to outcomes.** The most
severe matching outcome wins, and a context's fallback applies where no
rule matches. A document's **classes**, such as `privacy`, let rules treat
kinds of document differently.

> **Why.** What staleness means depends on what is being built. An internal
> staging build can tolerate a privacy policy that lags; a production
> deploy cannot.

**`notice` sits between passing and warning.**

> **Why.** It shows work that is approaching without painting every
> unrelated pull request red, which would teach people to ignore red.

**`fresh` always passes, and no rule can target it.** A context need not
say so.

**Errors are never downgraded.** A context decides about facts. It cannot
soften a configuration error, an integrity error such as a broken seal, or
a failure to evaluate.

> **Why.** A broken configuration must never look like an ordinary warning.

**The built-in `default` context** passes `fresh`, gives `missed` a notice
and warns on everything else. A project that declares its own `default`
replaces it entirely.

> **Why.** Full replacement is what makes gradual adoption possible: a
> project can give `unattested` a notice while it works through documents
> that existed before the tool did.

`unclaimed` is called so because "untracked" already means something else
in git.

## Gates

`check --fail-on warn|fail` sets the threshold. CI runs it twice, as two
gates:

- the **report gate** stops on a warning or a failure. It answers "is
  anything wrong?";
- the **continue gate** stops only on a failure. It answers "may the build
  go on?".

> **Why.** Most CI systems cannot show a warning: a job is green or red. A
> red report prompts the work, while the continue gate still lets a build
> through that nothing has failed.

| Exit | Meaning |
|---|---|
| 0 | Nothing reaches the threshold |
| 1 | A warning, at the `warn` threshold |
| 2 | A failure |
| 3 | A configuration or integrity error |
| 4 | The evaluation could not complete |
| 64 | Invalid usage |

> **Why.** An error has its own codes so that a script cannot mistake it
> for a document that is merely late, and "could not evaluate" is kept
> apart from "evaluated and found a problem".

**Both gates judge the same instant.** The recipe captures the time once
and passes it to both with `--at`, so that a deadline which passes between
the two runs cannot make them disagree.

**A scheduled run catches deadlines that pass without a commit.**

**`check` is the only gate.** A stale or failing document does not change
the exit code of `diff`, `explain` or `review`.

**Source Sentry sends no notifications.** CI and the forge do that already,
and doing it here would mean credentials, retries and code for each
service.

## Where declarations live

**A document is declared in a lockfile or in its own frontmatter, never
both.** Two declarations for one document are an error (`double-claim`).

> **Why.** A lockfile keeps metadata out of the document; frontmatter keeps
> it beside the content. Both are useful, but if one could silently shadow
> the other, nobody could tell which one is in force.

**Lockfiles are per directory.** A `source-sentry.lock` governs its
directory and everything below it, down to the next lockfile.

> **Why.** One central file would be a merge conflict waiting for every
> pull request that touches documentation.

**Lockfiles are written by the commands, as sorted JSON with one value per
line.** They are not meant to be edited.

> **Why.** Machine-written, sorted output makes a small, predictable diff
> in review, and two people's changes rarely touch the same lines. When a
> merge does conflict, keep both sides and run `check`: conflicts are never
> settled by picking the newer timestamp.

**A PDF is tracked through a lockfile.** How a format is read is set by its
**adapter**: `md-utf8` reads and writes frontmatter, `opaque` does not.

> **Why.** A PDF is usually replaced wholesale by a fresh export, which
> would erase anything embedded in it. A lockfile survives the export.

**A document used as a source by another must be tracked in a lockfile.**

> **Why.** Recording a review in frontmatter rewrites the document itself.
> If that document were a source, every review of it would make the
> documents that depend on it stale.

**Staleness passes downstream** as `upstream-stale`, and dependency cycles
are errors.

> **Why.** Without it, a decision could look fresh while the findings it
> rests on are stale.

**A lockfile-tracked document is still read for a declaration of its own.**

> **Why.** Otherwise a competing declaration in its frontmatter would go
> unnoticed.

Updating a declaration in frontmatter leaves every other key, the text and
the line endings exactly as they were.

## Paths

In a declaration, a bare path starts beside the document, `/` at the
document's project root, and `:/` at the repository root. On the command
line, relative paths start in the current directory, and `-C` changes it.

> **Why.** A leading slash makes root-relative paths explicit, and `:/`
> and `-C` mean what they mean in git, which users already know.

A source may lie outside its document's project, but never outside the
repository.

> **Why.** Naming a source claims no authority over it, so there is no
> reason to confine sources to a project.

Paths are stored in one canonical spelling, with `/` separators.

> **Why.** If `a/../b` and `b` were different names, one document could be
> declared twice without the double claim being seen, and results could
> differ between machines.

Paths are matched with globs, and series versions with regular expressions.

> **Why.** Globs are what `.gitignore` taught everyone, and the punctuation
> and anchoring of regular expressions are error-prone for paths. Parsing a
> version needs a capture group, which only a regular expression has.

## Patterns

**A pattern expands into files that are each reviewed at their own
version.** A file that newly matches is a source that was never reviewed; a
file that no longer matches is a change too.

> **Why.** Patterns give finer control than naming a directory, and new
> files are picked up without anyone remembering to declare them.

**A pattern is reviewed as a whole.** `attest --source` selects whole
sources, never single files of a pattern.

**A pattern that matches nothing is only a warning** (`empty-pattern`).
`track` and `attest` refuse a new pattern that matches nothing, but a
pattern whose last file was deleted is an ordinary change to acknowledge.

> **Why.** If an empty pattern were an error, the removal of its last file
> could never be reviewed.

A broad pattern makes many reviews for small changes, and reviews that come
too often stop being read. Prefer the narrowest pattern that covers what
the document describes.

## Records, seals and retirement

**A record is sealed explicitly.** `seal` stores the record's content
version; from then on, any difference is an integrity error
(`broken-seal`). `attest` can never seal, nor seal again.

> **Why.** If an ordinary review could set the seal, the routine answer to
> "this changed" would overwrite the very protection that reports it.

**A record is never corrected in place.** Restore it from git. If it is
wrong, write a new record that supersedes it.

**A sealed record's sources are frozen.** They say what it was based on, and
are no longer checked.

**Removing a document is explicit.** `retire` replaces its lockfile entry
with a tombstone that keeps its seal. A record deleted without one is an
integrity error.

> **Why.** The removal of a record should be as visible as the record was.
> Tombstones sit in their own section, so a path can be used again later.

A document declared in its own frontmatter is not retired: deleting the key
or the file ends its tracking.

## Series

A **series** groups versioned files, such as `audit-2026-03-01.md`, by a
pattern that captures the version: a whole number, a dotted number or a
date.

**Versions are compared as values, not as text.**

> **Why.** As text, `v0.0.10` sorts before `v0.0.9`. And `v0.3` equals
> `v0.3.0`, so that a duplicate cannot hide behind a trailing zero.

**A series is `living` or `records-only`.** In a living series the newest
entry is documentation and its predecessors are records. In a records-only
series every entry is a record, as for recurring audits.

**A superseded entry waits for its seal** (`awaiting-seal`) until someone
seals it, and a sealed predecessor never becomes live again, even if its
successor is deleted.

**A new entry is registered with `track`,** which copies its predecessor's
declaration without any review. `check` points to the command but never
registers anything itself.

**A cadence on a series asks for one completed entry per window.** An entry
completes once: a record when its seal is committed, a living entry when
all its sources are first reviewed. A window that closes without a
completion is `overdue`; once a later entry completes, the skipped window
becomes `missed`.

> **Why.** Counting completions accepts work finished early, refuses to
> count a file that merely exists, and keeps later changes from rewriting
> what was done on time. `missed` is kept apart from `overdue` so that
> skipped work is neither forgotten nor a permanent block.

## Discovery

**A directory with a lockfile is a documentation root,** and so is one
whose config says `root: true`. In a root, an eligible file that nothing
tracks is `unclaimed`. By default the eligible files are `.md`, `.txt` and
`.pdf`.

> **Why.** It catches the new page nobody declared, the new version of a
> series, and the declaration deleted by accident.

## Configuration

**Configuration is strict YAML.** Unknown keys, duplicate keys, several
documents in one file, merge keys, unsupported tags and the old booleans
such as `yes` and `no` are errors, each naming the file, the setting and
the correction.

> **Why.** YAML was chosen to match frontmatter, and its loose corners are
> where a policy file silently means something other than what was
> written. Merge keys would also be a second way to combine settings beside
> imports.

**Settings resolve from the outside in:** the built-in defaults, the
project's config and its imports, each directory config down to the
document, then the document's own declaration. `inherit: false` cuts the
directory configs above.

> **Why.** Settings are inherited so that they are written once. The cut
> exists so that a directory can start clean without having to remember
> and override every choice above it.

**Presets hold settings shared by documents of one nature,** and are
brought in by name with `imports`.

**Classes and contexts are declared in one place,** the project's config.

> **Why.** A preset is then valid or invalid on its own, whoever imports
> it.

**Settings never live in lockfiles.** Lockfiles are written by commands;
policy is written by people. `explain` shows where each effective setting
comes from.

## Projects

**A directory with a tracked `.source-sentry/source-sentry.yaml` is a
project,** and the repository root always is one. Projects nest, and a path
belongs to its nearest project.

**A project inherits nothing from the project around it.** Projects share
settings only by importing a preset by name.

> **Why.** Nothing arrives unannounced, and a project taken out of its
> repository cannot silently lose a setting it never named.

**`check` covers one project; `review` walks into nested ones.**

> **Why.** `check` is a gate, so it judges a project exactly as that
> project would be judged on its own. `review` serves a person, who may
> want to go through everything from the top.

**A document belongs to the project its path lies in,** and its declaration
lives there. When a source is a document of another project, only that
document's freshness is read, never the other project's contexts.

> **Why.** Freshness is a fact and depends on no policy, so it can cross a
> project boundary. Outcomes are policy, and stay inside.

## Remote documents

A document need not be a file in the repository. A **remote document** is
tracked by identity:

- `abstract:<name>` for anything at all, such as a tutorial video, under a
  name chosen in the repository;
- `google-drive:<id>`, `canva:<id>` or `miro:<id>` for a file on a
  supported service.

> **Why.** A video can depend on code without Source Sentry understanding
> video. The identity is a stable id rather than a URL, because one file
> has many URLs.

Remote documents are kept in one lockfile per **owner**, under
`.source-sentry/remotes/`.

> **Why.** Conflicts follow the people who edit, not the services that
> host, and a file per owner lets `CODEOWNERS` protect, for example, the
> legal team's reviews.

Source Sentry does not contact the services. Whether a remote document
still exists is unknown to it, and a remote document cannot be a record or
a member of a series. Owner names are labels for reports, not identities.

## What changes files

**`check` changes nothing.** It writes no file, does not touch the index,
creates no git object and fetches nothing.

> **Why.** A check that rewrote lockfiles would leave CI with a dirty
> tree, and a gate must be safe to run anywhere, any number of times.

**A command that changes files prints its plan and asks.** `--yes` answers
the question, `--dry-run` prints the plan only, and without a terminal or
`--yes` the command refuses.

> **Why.** Nothing should change silently, least of all in CI.

**A change is applied completely or not at all,** and a refusal says what is
wrong and what to do. `--yes` never overrides a refusal.

**A file that a command creates is marked intent-to-add,** and the command
says so. This is the only change Source Sentry ever makes to the index.

> **Why.** Only tracked files count, so a new lockfile that git did not
> know would be invisible to the next `check`. Staging its content would
> stage something the user did not ask to stage; intent-to-add records the
> path and leaves the content to them.

There is no cache. `--no-cache` is accepted and reserved.

The command has no short alias: `ss` is already a networking tool.

## Trust

Source Sentry's guarantees hold as long as its policy is protected by
review. The policy is every file that decides what is checked and how
strictly: lockfiles, configs and presets, declarations in frontmatter, and
the CI workflow that chooses the context and the threshold. Protect them
with code owners and branch protection;
[`../recipes/`](../recipes/README.md#protected-policy-surfaces) shows how.

Within that boundary:

- a recorded review means that someone ran `attest` against those exact
  versions. It does not prove who, nor that they read carefully;
- reviewer and owner names are labels;
- the repository's history is taken as true.

Weakening the policy makes a check pass without making the documentation
right. Lowering a threshold, widening a grace period, removing a source or
retiring a document are policy changes, to be made on purpose and reviewed
as such.

## What is not supported

- Submodules, and files behind a clean filter other than Git LFS.
- Git LFS pointers that use extensions.
- Paths that are not valid UTF-8.
- Checking whether a remote document exists.
- Pre-release ordering of versions in a series.
- Signed or notarised binaries.
