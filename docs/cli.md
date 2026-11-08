# Command-line interface

```
source-sentry [--repo <path>] [--color <auto|always|never>] [--quiet] <command>
```

- `--repo` names the repository; by default, the current directory, searching
  upward as git does.
- `--color` defaults to `auto`, which respects `NO_COLOR` and redirected
  output.
- `--quiet` prints errors only.

Documents are named by path, relative to the current directory as in git, or
by remote identity: `abstract:onboarding-video`,
`google-drive:1a2B3c4D5e6F7g8H9i0J`.

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
| `--no-cache` | Evaluate without the cache | the cache is used |
| `--format` | `human`, or `json` as in `structure/report.example.json` | `human` |

The exit codes are 0 below the threshold, 1 for warnings, 2 for failures,
3 for configuration or integrity errors, 4 when the evaluation could not
complete, and 64 for invalid usage.

The report gate and the continue gate are two invocations, which differ only
in `--fail-on`. Both receive the same `--at`, captured once by the CI recipe.

Human output, sketched:

```
Context production · 2026-11-07 18:00 UTC · working tree on 4f3c2a1

fail  docs/privacy.md              stale, overdue (due 2026-11-03)
      db/schema.sql changed in 7d2e9b0 on 2026-10-20
      production: privacy + stale → fail (.source-sentry/source-sentry.yaml:24)
warn  docs/onboarding.md           unattested since 2026-10-30
note  audits: security-audits      missed the window ending 2026-09-01

warning  empty-pattern  docs/source-sentry.lock:31  `/db/legacy/*.sql` matches no tracked file

1 failing · 1 warning · 1 notice · 9 passing · exit 2
```

`explain <document|path|series>` prints one subject in full: its effective
settings and where each comes from, its facts, the rules that matched, the
timing evidence and its next deadline. It takes the same
`--context`, `--rev` and `--at` as `check`.

## Reviewing

```
source-sentry diff <document> [--source <path-or-pattern>]... [--format <human|json>]
source-sentry attest <document> [--source <path-or-pattern>]...
                     [--expect <path@oid|path@absent>]... [--reviewer <name>]
                     [--reread] [--dry-run] [--yes]
```

`diff` shows each source's changes since its attestation, and ends with the
`attest` command line that binds exactly what was shown, expectations
included. `--reviewer` defaults to git's `user.name`.
`--reread` also records that the whole document was reread.

## Records

```
source-sentry seal <document> [--by <name>] [--dry-run] [--yes]
source-sentry retire <document> [--by <name>] [--dry-run] [--yes]
```

## Tracking

```
source-sentry track <document> [--on-file | --lockfile [<directory>] | --remote <owner>]
                    [--url <url>] [--source <path>]... [--pattern <glob>]...
                    [--class <name>]... [--owner <id>]...
                    [--grace <days>] [--cadence <dtstart> <rrule> | --no-cadence]
                    [--dry-run] [--yes]
source-sentry track <document>... --from <placement> --to <placement> [--dry-run] [--yes]
source-sentry init [<path>... | --all] [--class <name>]... [--owner <id>]... [--dry-run] [--yes]
source-sentry migrate [--dry-run] [--yes]
```

- **The default target** of `track` is the nearest lockfile, or a new one in
  the document's directory. A remote identity requires `--remote`.
- **A placement** is written `on-file`, `lockfile:<directory>` or
  `remote:<owner>`.
- **`init` without paths** only previews roots, claims and candidates. With
  paths, or `--all`, it registers them as unattested.

## Confirmation

A command that changes files prints its plan first. On a terminal it then
asks for confirmation; `--yes` skips the question, and `--dry-run` prints the
plan only. Without a terminal and without `--yes`, it refuses, so CI never
changes files silently.

The binary is named `source-sentry` only, because `ss` is already the
socket-statistics command on Linux.
