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

- The first release of Source Sentry: the `source-sentry` command and the
  `source-sentry-core` and `source-sentry-git` libraries.
