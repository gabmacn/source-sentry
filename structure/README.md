# File formats

Examples of every file that Source Sentry reads or writes. The YAML examples
explain themselves in comments. JSON allows no comments, so the lockfile
formats are explained below.

| File | Where the real file lives |
|---|---|
| `example.yaml` | The frontmatter of a Markdown document |
| `lockfile.example.json` | `<directory>/source-sentry.lock` |
| `remote-lockfile.example.json` | `.source-sentry/remotes/<owner>.lock` |
| `repository-config.example.yaml` | `.source-sentry/source-sentry.yaml` |
| `directory-config.example.yaml` | `<directory>/source-sentry.yaml` |
| `preset.example.yaml` | `.source-sentry/presets/<name>.yaml` |
| `remote-config.example.yaml` | `.source-sentry/remotes/<owner>.yaml` |
| `report.example.json` | The output of `check --format json` |

Every oid and every name in the examples is made up.

The examples fit together. The directory config imports the preset as
`logs`, so the preset belongs at `.source-sentry/presets/logs.yaml`; it
imports nothing else. The remote config and remote lockfile pair as
`.source-sentry/remotes/legal.yaml` and `legal.lock`. The frontmatter and the
local lockfile show two ways to track a document, and a document uses only
one. To try them, add the documents and sources they name; the made-up oids
then show as stale sources or a broken seal.

## Lockfiles

Source Sentry writes lockfiles, so you should not need to edit them. They are
meant to be easy to read in a pull request, and to merge.

A lockfile has three keys:

- **`documents`**: the documents tracked here. In a lockfile beside your
  files, each document is keyed by its path relative to the lockfile's
  directory.
- **`retired`**: documents that were deliberately removed. A record keeps its
  seal here, so its removal stays visible, and an entry of a `living` series
  keeps its `completed`, so the window it credited stays credited.
- **`schema`**: the version of the lockfile format.

Unknown fields and duplicate object keys are errors. Local document keys
must already be canonical: no leading or trailing `/`, `.`, `..` or empty
components, and no file named `source-sentry.lock`. Only the nearest ancestor
lockfile may list or retire a document. A retired path may be reused, so a key
may appear in both `documents` and `retired`.

Each document has the same fields as a declaration in frontmatter
(`example.yaml`), plus **`seal`**: `at`, `by` and `oid`. A sealed document is
a record, which must never change.

**`completed`**, also allowed in frontmatter, holds `at` and `by` for a
living-series entry's first complete source review. The preset example
explains how committed completion and seal markers credit series windows.

Keys are sorted alphabetically and every value sits on its own line, so two
people's changes rarely touch the same lines.

## Remote lockfiles

A remote lockfile tracks documents that live outside the repository. There is
one per owner, beside an optional config of the same name that sets defaults
for its documents. The format matches a local lockfile, with these
differences:

- **Keys are identities rather than paths.** `google-drive:<file-id>`,
  `canva:<design-id>` and `miro:<board-id>` name a file on a supported
  service. `abstract:<name>` names anything else, such as a video, under a
  name you choose.
- **`url`** may be added to any document, for convenience. It never changes
  the document's identity.
- **`seal` and `completed` are forbidden**, including in `retired` entries:
  remote documents cannot be records or series members.

Sources of remote documents must start with `/`, because a remote document
has no directory to be relative to.

## The JSON report

`check --format json` prints one JSON object, also when the configs are
invalid. A failure before evaluation, such as a revision that does not
resolve, and invalid command-line usage print an error instead, with no JSON.
The object's keys are sorted, as in lockfiles, and `schema` versions the
format for tools that read it, such as CI jobs.

- **`at`, `revision`, `context` and `threshold`** say what was evaluated, and
  how. `exit-code` and `gates` give both gates' answers; the continue gate
  fails only on a failure.
- **`subjects`** are the things judged: `document`, `unclaimed` and `series`.
  Each subject has:
  - its `facts`;
  - its `outcome`;
  - a `judgement` per fact, whose `rule` is `null` for the context's fallback
    or for `fresh`, which always passes;
  - for documents, the effective `settings`, each with its origin, the
    `sources` with their state and clock, and the next `deadline`.
- **`origin`** is one of three kinds:
  - `config`: a file, line, import chain and scope. The scope is `repository`,
    `directory:<path>` or `remote:<owner>`, written like `track`'s placements;
  - `declaration`: a file and field;
  - `built-in`.
- **`diagnostics`** have a `severity` (`warning`, `configuration-error`,
  `integrity-error` or `operational-error`), a stable `code`, a `message` and
  a `location`; `related` lists other involved locations, sorted.

Other JSON variants:

- An unjudged subject has `judgement: null` and `outcome: null`.
- Documents may add `record` (seal check), `awaiting-seal` (series reference),
  `stale-upstreams` (direct upstream documents) or `unattested` (review
  clock).
  Pattern members add `pattern`, resolved against the repository root.
- A clock is `{commit, since}` when started, `null` when idle, `"not-started"`
  for working-tree-only changes, `"unknown"` when the attested oid never
  appears in history, or `"failed"` when dating fails.
- `revision` is `{kind: "commit", commit: <oid>}` for a committed snapshot;
  a working tree uses `{kind: "working-tree", head: <oid or null>}`.

When a config cannot be loaded, nothing is evaluated: the report has
`context: null`, an empty `subjects` array and an `invalid-config` diagnostic
for each config error, except invalid cadences, which use `invalid-schedule`.
The exit code is 3.

## Diagnostic codes

Each code has a fixed severity. Warnings do not affect the exit code;
configuration and integrity errors exit 3, and operational errors exit 4.
Context rules cannot downgrade errors.

| Code | Severity | Meaning |
|---|---|---|
| `invalid-lockfile` | configuration-error | Lockfile cannot be read. |
| `invalid-declaration` | configuration-error | On-file declaration cannot be read. |
| `double-claim` | integrity-error | A document has multiple declarations. |
| `retired-in-another-lockfile` | configuration-error | Remote retirement conflicts across owners. |
| `outside-authority` | configuration-error | A nearer lockfile owns the path. |
| `missing-document` | configuration-error | A listed document is absent. |
| `missing-record` | integrity-error | A sealed record is absent without retirement. |
| `undeclared-class` | configuration-error | A class is not declared. |
| `invalid-source-path` | configuration-error | A source or member path cannot resolve. |
| `missing-source` | configuration-error | An explicit live source is absent. |
| `invalid-pattern` | configuration-error | A source pattern cannot compile or resolve. |
| `self-dependency` | configuration-error | A document names itself as a source. |
| `empty-pattern` | warning | A source pattern matches no tracked path. |
| `on-file-source` | configuration-error | A source document needs lockfile tracking. |
| `dependency-cycle` | configuration-error | Document dependencies form a cycle. |
| `series-overlap` | configuration-error | A file matches multiple series. |
| `duplicate-version` | configuration-error | Series entries share a parsed version. |
| `invalid-version` | configuration-error | A series version does not parse. |
| `case-collision` | warning | Tracked sibling names differ only in case. |
| `invalid-config` | configuration-error | Config loading failed. |
| `unknown-context` | configuration-error | The requested context is not declared. |
| `invalid-context` | configuration-error | A declared context has invalid rules. |
| `unsupported-defaults-version` | configuration-error | This build lacks the pinned default policy. |
| `broken-defaults` | configuration-error | The built-in default context is unreadable. |
| `invalid-time-zone` | configuration-error | The time zone is not an IANA name. |
| `invalid-schedule` | configuration-error | Cadence is invalid or has no required due date. |
| `submodule` | configuration-error | A source is an unsupported submodule. |
| `sealed-head` | configuration-error | The newest living-series entry is sealed. |
| `missing-head` | configuration-error | The newest living entry is missing or retired. |
| `broken-seal` | integrity-error | A record differs from its seal. |
| `unsupported-filter` | operational-error | An external clean or process filter applies. |
| `unsupported-lfs-extension` | operational-error | An LFS pointer uses unsupported extensions. |
| `crlf-in-index` | operational-error | Indexed CRLF conflicts with normalisation. |
| `shallow-history` | operational-error | A clock search reaches shallow history. |
| `future-dated` | operational-error | A clock exceeds the future-date tolerance. |
| `date-out-of-range` | operational-error | A calculation leaves the supported calendar. |
