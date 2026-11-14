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

- The facts `due` and `upstream-due`. A document is due from the first
  instant of its review day, and a document is upstream-due when a document
  it uses as a source is due. Every overdue subject is also due.
- An uncommitted change is judged as if committed at the instant of the
  check to decide whether its document is due, and its deadline is reported
  as provisional (`provisional` in JSON).
- The JSON report's `deadline` gains `due-from` and `provisional`, and a
  document gains `due-upstreams`.
- `review --all` asks, in the line mode, about documents that are stale but
  not due yet. The full screen lists them with a `not due` tag.

### Changed

- The built-in default context, version 1, gives `stale` and
  `upstream-stale` a notice: a stale document warns from its due day, not
  from its change. A document without a schedule is due at once, so it
  still warns at once. A project that defines its own `default` keeps its
  own rules.
- `review`'s `a` records only the due documents; its confirmation offers to
  include those not due yet. The line mode queues only due documents unless
  `--all` is given or the document is named.
- Human output leaves out facts implied by a more urgent one, and labels
  deadlines `review by` (`entry by` for a series): `overdue (missed review by
  2026-11-03)` rather than `stale, overdue (due 2026-11-03)`. `explain`
  names a series window's date `cadence date`.
- The CI recipes' scheduled runs fail their report gate on warnings, so that
  a review day notifies someone.
- The Agent Skill tells agents to give a cadence to broad documents they
  track for the first time, to propose one for existing documents, and to
  leave a document that is not due yet for its review day after correcting
  what their change contradicts.

## [0.1.0-rc.1] - 2026-11-12

### Added

- The first release candidate of Source Sentry: the `source-sentry` command
  and the `source-sentry-core` and `source-sentry-git` libraries.
