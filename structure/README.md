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

## Projects

A tracked `.source-sentry/source-sentry.yaml` marks its containing directory
as a project root. The repository root is always a project, even without
that file. Projects can nest, and each path belongs to its nearest project.
Configs, document scans, roots, series and unclaimed files stop at nested
project boundaries. A project's own `.source-sentry/` supplies its version,
time zone, classes, contexts, presets and remote documents. Enclosing
configs supply no inherited settings.

`check` and `init` cover the project containing the effective starting
directory. Document commands use the document's project. `review` traverses
the starting project and its nested projects, each under its own configs;
a project missing the selected context uses its own `default`, announced
at the start of the session. Use `-C <directory>` to change the starting
directory, as in git.

Source spellings have two roots: `/` means the document's project root,
and `:/` means the repository root. Relative declarations start beside
the document. Sources can leave their project, but never the repository.
Remote documents belong to the project containing their owner lockfile,
and use rooted sources. A source document in another project contributes
freshness from its own declarations and schedules, without applying its
contexts.

Projects share settings only through explicit preset imports. An import
`common` resolves first to the project's `.source-sentry/presets/common.yaml`,
then to each enclosing project's preset directory, outwards. The nearest
file wins. Preset patterns are anchored at the importing config.

A preset can contain `define` for classes and contexts, as well as ordinary
settings. Only a project's own `.source-sentry/source-sentry.yaml` may import
a defining preset directly. Directory configs, paired remote configs and
other presets cannot import one. Two explicit definitions of the same class
or context are an error naming both files; nothing silently overrides a
definition. One explicit `default` context replaces the built-in default.

Local declarations must live inside the project containing their document.
After `git mv` across a project boundary, `rename` previews every source
rewrite and effective setting change. A `/` source left outside the new
project becomes `:/`, and reviews are preserved. If a new marker leaves
declarations in an outer lockfile, `check` names a `track --from/--to` repair
command. The destination must define the declaration's explicit classes.
Remote owners in another project can be selected with
`remote:<owner>@<project-directory>`; the directory must be a project root.

JSON subjects and diagnostic locations carry `project`, the repository
path of their project root. The repository project is written `.`. Paths
inside each subject or diagnostic location are relative to that project;
external sources use `../`. Each related diagnostic location identifies
its own project independently. The report schema remains 1.

## Lockfiles

Source Sentry writes lockfiles, so you should not need to edit them. They are
meant to be easy to read in a pull request, and to merge.

A lockfile has three keys:

- **`documents`**: the documents tracked here. In a lockfile beside your
  files, each document is keyed by its path relative to the lockfile's
  directory.
- **`retired`**: documents that were deliberately removed. Each key holds a
  non-empty list of tombstones, oldest first by `retired.at`, even when there
  is only one. Equal instants retain their written order. A record keeps its
  seal here, so its removal stays visible, and an entry of a `living` series
  keeps its `completed`, so each window it credited stays credited.
- **`schema`**: the version of the lockfile format.

Unknown fields and duplicate object keys are errors. Local document keys
must already be canonical: no leading or trailing `/`, `.`, `..` or empty
components, and no file named `source-sentry.lock`. Only the nearest ancestor
lockfile may list or retire a document. A retired path may be reused, so a key
may appear in both `documents` and `retired`. Retiring it again adds another
tombstone, keeping every earlier seal and completion. Empty lists and lists
that are not in chronological order are errors. The schema remains 1.

Each document has the same fields as a declaration in frontmatter
(`example.yaml`), plus **`seal`**: `at`, `by` and `oid`. A sealed document is
a record, which must never change.

**`completed`**, also allowed in frontmatter, holds `at` and `by` for a
living-series entry's first complete source review. The preset example
explains how committed completion and seal markers credit series windows.

**`renamed`**, also allowed in frontmatter, is an optional list of explicit
former document paths, newest rename last. Each entry has `from`, `at` and
`by`; `by` is a name or `null`. In `from`, `/docs/old.md` starts at the
current declaration's project root, and `:/other/docs/old.md` names a former
path outside that project from the repository root. Components are canonical,
without `.` or `..`. Project moves re-express every earlier entry as well as
sources, preserving the targets. The list follows command order, even when
the recorded timestamps move backwards. The schema remains 1.

Evaluation follows each former identity only before the commit introducing
its rename marker. Obligation clocks, seals and living-series completion
credits retain their original history, including before a rename is
committed. Stored timestamps never date these clocks. Unreadable historical
declarations can only advance an obligation's deadline; they grant no credit.
Project-rooted provenance also resolves in an exported project's history.

Each pattern source may carry **`reviewed`**, with `at` and `by`, as well as
its members' reviews. Attesting a pattern writes the same review on the
pattern and all current members, and drops removed members. Sources are
selected as whole paths or patterns; individual pattern members cannot be
selected. Attesting a path that also matches a pattern leaves that pattern
and its members unchanged. Unselected sources keep their reviews.

The pattern's review remains when its last member is removed. It records
review evidence without recording a whole-document reread, and does not
attest a new member or replace any member's oid and review. Older lockfiles
may omit it, including on patterns with reviewed members; those member
reviews still count as review evidence. The schema remains 1. Frontmatter
uses the same optional field.

Keys are sorted alphabetically and every value sits on its own line, so two
people's changes rarely touch the same lines.

## Series registration

Series patterns group files by parsed version, as `preset.example.yaml`
shows. A registered successor stores an ordinary declaration with its
inherited settings and re-expressed sources. It starts without source
attestations, pattern reviews or members, `reviewed`, `seal`, `completed`
or `renamed`; there is no inheritance link in the file format.

See [`docs/cli.md`](../docs/cli.md#tracking) for predecessor selection,
registration options and the behaviour of `track` and `init`.

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
- **`retired` keys hold the same non-empty, ordered lists of tombstones.**
- **`seal` and `completed` are forbidden**, including in every tombstone:
  remote documents cannot be records or series members.

Sources of remote documents must start with `/` or `:/`, because a remote
document has no directory to be relative to.

## The JSON report

`check --format json` prints one JSON object, also when the configs are
invalid. A failure before evaluation, such as a revision that does not
resolve, and invalid command-line usage print an error instead, with no JSON.
The object's keys are sorted, as in lockfiles, and `schema` versions the
format for tools that read it, such as CI jobs.

- **`at`, `revision`, `context` and `threshold`** say what was evaluated, and
  how. `exit-code` and `gates` give both gates' answers; the continue gate
  fails only on a failure.
- Each subject and diagnostic location includes **`project`**, the repository
  path of its project root (`.` for the repository root). Paths in that
  subject or location are relative to its project, with `../` for sources
  outside it.
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
- Unclaimed series entries add `inherit-from`, the project-relative path of the
  newest earlier entry with a live declaration that registration would
  copy. The field is omitted when there is no such predecessor. Human
  `check` and `explain` output also name the `source-sentry track <path>`
  command to register the entry.
- Documents may add `record` (seal check), `awaiting-seal` (series reference),
  `stale-upstreams` (direct upstream documents) or `unattested` (review
  clock).
  Pattern members add `pattern`, resolved against their project's root.
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
| `lockfile-source` | configuration-error | A path source names a Source Sentry lockfile; recording reviews rewrites it. |
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
