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

## Lockfiles

Source Sentry writes lockfiles, so you should not need to edit them. They are
meant to be easy to read in a pull request, and to merge.

A lockfile has three keys:

- **`documents`**: the documents tracked here. In a lockfile beside your
  files, each document is keyed by its path relative to the lockfile's
  directory.
- **`retired`**: documents that were deliberately removed. A record keeps its
  seal here, so its removal stays visible.
- **`schema`**: the version of the lockfile format.

Each document has the same fields as a declaration in frontmatter
(`example.yaml`), plus two more:

- **`seal`**: `at`, `by` and `oid`. A sealed document is a record, which must
  never change.
- **`completed`**: `at` and `by`. It marks when the current entry of a
  versioned series was first fully reviewed, which counts towards the series'
  schedule.

Keys are sorted alphabetically and every value sits on its own line, so two
people's changes rarely touch the same lines.

## Remote lockfiles

A remote lockfile tracks documents that live outside the repository. There is
one per owner, beside an optional config of the same name that sets defaults
for its documents. The format matches a local lockfile, with two
differences:

- **Keys are identities rather than paths.** `google-drive:<file-id>`,
  `canva:<design-id>` and `miro:<board-id>` name a file on a supported
  service. `abstract:<name>` names anything else, such as a video, under a
  name you choose.
- **`url`** may be added to any document, for convenience. It never changes
  the document's identity.

Sources of remote documents must start with `/`, because a remote document
has no directory to be relative to.

## The JSON report

`check --format json` prints one JSON object. Its keys
are sorted, as in lockfiles, and `schema` versions the format for tools that
read it, such as CI jobs and the agent manager.

- **`at`, `revision`, `context` and `threshold`** say what was evaluated, and
  how. `exit-code` and `gates` give both gates' answers; the continue gate
  fails only on a failure.
- **`subjects`** are the things judged: `document`, `unclaimed` and `series`.
  Each subject has:
  - its `facts`;
  - its `outcome`;
  - a `judgement` per fact, naming the rule that decided it, or `null` for
    the context's fallback;
  - for documents, the effective `settings`, each with its origin, the
    `sources` with their state and clock, and the next `deadline`.
- **`origin`** is one of three kinds:
  - `config`: a file, line, import chain and scope. The scope is `repository`,
    `directory:<path>` or `remote:<owner>`, written like `track`'s placements;
  - `declaration`: a file and field;
  - `built-in`.
- **`diagnostics`** have a `severity` (`warning`, `configuration-error`,
  `integrity-error` or `operational-error`), a stable `code`, a `message` and
  a `location`.
