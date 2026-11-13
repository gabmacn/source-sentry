---
name: source-sentry
description: Keeps documentation in step with the code it describes, using the Source Sentry command-line tool. Use when `source-sentry check` fails or warns, when a change touches files that tracked documents describe, when adding, moving, renaming or deleting documentation, or when a task mentions source-sentry.lock, stale or unattested documents, records, seals or attestations.
license: MIT
---

# Source Sentry

Source Sentry tracks documents against the files they describe, their
**sources**. When a source changes, every document that names it becomes
**stale** until someone reads the change, updates the document if needed, and
records that review with `attest`. Each review is bound to the exact versions
of the sources that were read, so a recorded review is a claim that a reviewer
checked those versions.

Declarations live in `source-sentry.lock` files (JSON) or in a document's
`source-sentry` frontmatter key. Policy lives in `source-sentry.yaml` files
and in `.source-sentry/`. The tool's own reference is `docs/cli.md` in the
Source Sentry repository, and `source-sentry help <command>` describes each
command.

## Rules

1. **Never edit lockfiles, files under `.source-sentry/`, or a document's
   `source-sentry` frontmatter key by hand.** Use the commands below. They
   validate every change, keep the format stable for review, and refuse
   changes that would leave the repository inconsistent. The one exception
   is resolving a merge conflict in a lockfile: keep the entries of both
   sides, then run `source-sentry check`.
2. **Never record a review you did not perform.** Before `attest`, read the
   changes that `diff` shows and the document itself, and correct the
   document where it no longer matches its sources.
3. **Attest only if the person you work for allows it,** and then name
   yourself: `--reviewer "<your name>"`. Without it, the review carries the
   name in the person's git configuration (`source-sentry.reviewer`, or else
   `user.name`). Never record a review under a person's name, and never use
   an email address. Name yourself the same way with `--by` when you `seal`
   or `retire`. Without permission, update the documents and give the person
   the `attest` commands to run.
4. **Never change a record.** A sealed document must never change. If it is
   wrong, write a new document that corrects it.
5. **Never weaken the policy to pass a check.** Do not lower `--fail-on`,
   raise a grace period, remove a source, or retire a document to make a
   warning go away, unless that is what the person asked for.
6. **Read refusals.** Every error says what is wrong and what to do. `--yes`
   only answers the confirmation question; it never overrides a refusal.

Paths typed on the command line start in the effective current directory;
`-C <path>` changes it, as in git. Source spellings use `/` for the
document's project root and `:/` for the repository root. Relative sources
in declarations start beside the document. Sources can leave their project
but never the repository. A tracked `.source-sentry/source-sentry.yaml`
marks a project root; the repository root always counts. `check` and `init`
cover only the starting project, document commands use the document's project,
and `review` traverses nested projects. Check each affected project with
`source-sentry -C <project> check`; an enclosing check excludes it.

## When `check` reports problems

```sh
source-sentry check                    # Read outcomes and diagnostics.
source-sentry explain <document>       # Why a document is judged as it is.
```

`check` exits 0 when no subject reaches `--fail-on` (default: `warn`),
1 for a subject's `warn` outcome at the `warn` threshold, 2 for a `fail`
outcome, 3 for a configuration or integrity error, and 4 when the evaluation
could not complete.

A diagnostic of severity `warning`, such as `empty-pattern`, never changes
the exit code. Exit 0 can still accompany diagnostic warnings or outcomes
below the selected threshold; read the report before deciding what needs
attention. Handle errors (3 and 4) first, then each listed document by its
facts:

- **stale** or **overdue**: a source changed since the last review. Review
  it as described below. Overdue means the deadline has passed; the remedy
  is the same.
- **upstream-stale**: a document used as a source is itself stale. Review
  that document first; this fact then clears.
- **unattested**: the document was never reviewed. Read the whole document
  against its sources, correct it, then record the review with
  `attest <document> --reread`.

### Reviewing a stale document

1. Run `source-sentry diff <document>`. It shows each source's changes since
   its last review and ends with an `attest` command.
2. Read the changes and the document. Edit the document until it is accurate
   again, or confirm that it already is.
3. Run the printed `attest` command, adding `--reviewer "<your name>" --yes`.
   Its `--expect` arguments bind exactly the versions you were shown. If
   `attest` refuses because a source changed since, run `diff` again.
4. Run `source-sentry check` again: editing a document can make the
   documents that use it as a source stale in turn.

`source-sentry review` walks the same steps interactively, but it needs a
terminal; agents use `diff` and `attest`.

### Diagnostics

| Code | What to do |
|---|---|
| `missing-source` | Restore the source if its deletion was accidental. If it is a tracked document that moved, run `rename <old> <new>` after `git mv` to preserve its reviews and update dependent sources. Otherwise remove the obsolete source with `untrack <document> --source <old path>`, add any replacement with `track <document> --source <new path>`, then review the document. |
| `empty-pattern` | A source pattern matches no file. Correct it with `track --pattern` and `untrack --pattern`. |
| `missing-document` | A tracked document is gone. If it was deleted on purpose, `retire` it; if it was moved, run `source-sentry rename <old> <new>` after `git mv`, keeping its reviews. |
| `broken-seal` | A record changed. Restore it from git, for example `git restore --source=<commit> -- <path>`. Never reseal it. |
| `double-claim` | A document is declared in two places. Ask which declaration is right before removing either. |
| `invalid-lockfile`, `invalid-declaration`, `invalid-config` | A file is malformed, usually by a hand edit or a bad merge. Restore it from git or repair the merge, then run `check`. |
| `shallow-history` | The clone lacks the history needed to date changes. Fetch it with `git fetch --unshallow`. |

For any other code, run `source-sentry explain` on the document and follow
the message. `structure/README.md` in the Source Sentry repository lists
every code.

## Changing documentation

- **A new document:** `source-sentry track <document> --source <path>
  --pattern <glob>`, naming every file it describes. It starts unattested:
  review it and record the review with `--reread`.
- **A new source for a tracked document:** `track <document> --source
  <path>`. The document is stale until the new source is reviewed.
- **A source that no longer applies:** `untrack <document> --source <path>`
  or `--pattern <glob>`.
- **Deleting a document:** `source-sentry retire <document>`, then delete
  the file. For a document declared in its own frontmatter, delete the file
  instead; its declaration goes with it.
- **Renaming a document:** move it with `git mv <old> <new>`, then run
  `source-sentry rename <old> <new>`. Both paths start in the current
  directory. The command keeps its reviews, seal and completion, re-expresses
  relative sources and updates documents that use it as a source. Check the
  plan, then run `check` for patterns that no longer match or changes to
  series membership. Commit an on-file declaration before moving it, so the
  command can verify that the declaration travelled with the file.
- **Moving a declaration** between a lockfile, the frontmatter and another
  lockfile: `track <document> --from <placement> --to <placement>`, where a
  placement is `on-file`, `lockfile:<directory>`, `remote:<owner>` or
  `remote:<owner>@<project-directory>`. A local declaration stays in its
  document's project; use `rename` after moving the document between projects.
  The qualified remote placement selects an owner in another project.
- **Sealing a record:** `source-sentry seal <document>`, only when the person
  asks for it: a sealed document can never change again.

Commands that change files print a plan, then ask for confirmation. Add
`--dry-run` to see the plan only, and `--yes` to confirm without a terminal.

## Changing code

When your change touches files that documents describe, run
`source-sentry check` before you finish. Review every document your change
made stale, as above, in the same change, so the documentation and the code
it describes are committed together.
