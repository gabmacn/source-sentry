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
