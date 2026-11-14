# Source Sentry

<p align="center">
  <img src="docs/images/the-sentry-ink.png#gh-light-mode-only" width="320" alt="The Sentry: a card showing a peacock-feather eye, in blue and cream on ink">
  <img src="docs/images/the-sentry-cream.png#gh-dark-mode-only" width="320" alt="The Sentry: a card showing a peacock-feather eye, in blue and ink on cream">
</p>

Tracks documentation and records against the repository sources they describe.
It supports documentation roots per product, and distinguishes documentation,
which must follow its sources, from records, which must not change once
written.

When a source changes, the documents that describe it become stale, and each
document gets a deadline set by its schedule. A context decides what staleness
means for a build: a privacy policy can block a production deploy while a
marketing page only warns.

Source Sentry does not judge whether a document is right. It tells which
documents a change may have made wrong, and records that a named reviewer
checked them against exact versions of their sources.
[`docs/how-it-works.md`](docs/how-it-works.md) explains the model and the
reason for each rule.

## Installing

Releases are published at
<https://git.gabmacn.com/gmn/source-sentry/releases> and copied, with the
same files, to <https://github.com/gabmacn/source-sentry/releases>. Versions
before 0.1.0 are release candidates. There are two ways to install Source
Sentry:

- **Prebuilt binaries** for Linux (x86_64 and aarch64), macOS (Apple silicon)
  and Windows (x86_64), attached to each release. Each comes in an
  archive named `source-sentry-<version>-<target>`, a `.tar.gz` for Linux and
  macOS and a `.zip` for Windows. It holds a folder of the same name with the
  binary, `LICENSE` and `THIRD-PARTY-LICENSES.txt`. Every release includes a
  `SHA256SUMS` file. Verify the archive before extracting it:

  ```sh
  sha256sum --check --ignore-missing SHA256SUMS        # Linux
  shasum -a 256 --check --ignore-missing SHA256SUMS    # macOS
  tar -xzf source-sentry-<version>-<target>.tar.gz
  ```

  On Windows, compare the output of `Get-FileHash -Algorithm SHA256` with the
  archive's line in `SHA256SUMS`, then extract it with `Expand-Archive`. The
  Windows binary needs no Visual C++ Redistributable.

  The binaries are not signed. macOS quarantines a binary downloaded through
  a browser, and refuses to run it until the mark is removed:

  ```sh
  xattr -d com.apple.quarantine source-sentry-<version>-aarch64-apple-darwin/source-sentry
  ```

  Downloads made with `curl` are not quarantined. The macOS archive is built
  separately, and can appear on a release later than the others.
- **The Cargo registry** at `git.gabmacn.com`. Add the registry to
  `~/.cargo/config.toml`:

  ```toml
  [registries.gmn]
  index = "sparse+https://git.gabmacn.com/api/packages/gmn/cargo/"
  ```

  Then install, with `--locked` so that the dependency versions that were
  tested are the ones used. Cargo installs the latest release; it picks a
  release candidate, such as `0.1.0-rc.3`, only when `--version` names it.

  ```sh
  cargo install --locked --registry gmn source-sentry
  ```

[`install/`](install/README.md) holds quicker ways to the prebuilt binaries:
install scripts for Linux, macOS and Windows that verify the checksum,
cargo-binstall support, a [proto](https://moonrepo.dev/proto) plugin that
pins a version per repository, and a [moon](https://moonrepo.dev/moon)
project, so that a monorepo's teams run `moon run source-sentry:review`
without installing anything by hand:

```sh
curl -fsSL https://git.gabmacn.com/gmn/source-sentry/raw/branch/main/install/install.sh | sh
```

`source-sentry completions <shell>` prints shell completions for bash, zsh,
fish, PowerShell and elvish.

Source Sentry runs `git` for some operations, so git must be on the `PATH`.
[`CHANGELOG.md`](CHANGELOG.md) lists the changes in each release.

## Five minutes with Source Sentry

[source-sentry-example](https://github.com/gabmacn/source-sentry-example) is
a small repository with Source Sentry already set up: `docs/guide.md`
describes the settings in `src/settings.py`, and records which version of
that file it was last reviewed against. Clone it, with Source Sentry
installed:

```sh
git clone https://github.com/gabmacn/source-sentry-example
cd source-sentry-example
source-sentry check
```

```
Context default · 2026-11-14 20:45:37 UTC · working tree on 4297608

2 passing · exit 0
```

Both documents match their sources. Now change the code: set `PORT = 9090`
in `src/settings.py`, and check again.

```
$ source-sentry check
Context default · 2026-11-14 20:45:37 UTC · working tree on 4297608

warn  docs/guide.md  due (review at once)
      src/settings.py changed in the working tree

1 warning · 1 passing · exit 1
```

Source Sentry does not know whether the guide is now wrong, only that a file
it describes has changed since its last review. `due (review at once)` comes
from the guide's schedule: with the default, a document must follow its
sources at once. `diff` shows what changed since that review:

```
$ source-sentry diff docs/guide.md
docs/guide.md · 1 file changed since attested

`../src/settings.py`
  src/settings.py  changed: 34255e9 → 7db3392
  --- src/settings.py (34255e9)
  +++ src/settings.py (7db3392)
  @@ -1,7 +1,7 @@
   """Settings of the example web service."""

   # The port the service listens on.
  -PORT = 8080
  +PORT = 9090

   # How many requests one client may make per minute.
   RATE_LIMIT_PER_MINUTE = 60

After the review, record exactly what was shown with:
  source-sentry attest docs/guide.md --expect src/settings.py@sha1:7db3392d28ef19e1e56a6db947a73c00d2e42511
```

The guide says port 8080, so update it to 9090, then record the review with
the command `diff` printed. `--expect` binds the review to the exact version
you read: if the file changed again in the meantime, `attest` refuses. On a
terminal it asks before writing; `--yes` skips the question.

```
$ source-sentry attest docs/guide.md --expect src/settings.py@sha1:7db3392d28ef19e1e56a6db947a73c00d2e42511 --yes
Attest docs/guide.md (docs/guide.md `source-sentry`)
  `../src/settings.py`
    src/settings.py  34255e9 → 7db3392
Reviewed by Ada at 2026-11-14 20:45:46 UTC
Matches the expectations:
  src/settings.py@sha1:7db3392d28ef19e1e56a6db947a73c00d2e42511

Files:
  write   docs/guide.md
Applied: 1 file changed.

$ source-sentry check
Context default · 2026-11-14 20:45:47 UTC · working tree on 4297608

2 passing · exit 0
```

The review is recorded in the guide's frontmatter, with the reviewer's name,
the time and the version of `src/settings.py` it was checked against, so it
travels with the commit. In CI, the same check runs as two gates: the example
repository's [pull request #1](https://github.com/gabmacn/source-sentry-example/pull/1)
makes this change without updating the guide, and fails the report gate
while the required continue gate passes. Its `production` context goes
further, and blocks a deploy while `docs/privacy.md` does not match
`src/storage.py`. [`recipes/`](recipes/README.md) explains both.

`source-sentry review` walks through every pending document on a terminal,
showing each diff and recording each review in turn.

## Using

```sh
source-sentry check                     # Evaluate the working tree.
source-sentry check --context production --fail-on fail
source-sentry diff docs/privacy.md      # What changed since the last review.
source-sentry review                    # Review pending documents on a terminal.
source-sentry attest docs/privacy.md    # Record a review.
source-sentry explain docs/privacy.md   # Why a document is judged as it is.
source-sentry rename docs/old.md docs/new.md # Update records after git mv.
```

Projects nest: a tracked `.source-sentry/source-sentry.yaml` marks a project
root, and the repository root is always a project. A path belongs to its
nearest project. Each project uses its own configs, sharing only by explicit
preset imports. `check` and `init` cover the starting project; `review`
traverses its nested projects. Document commands use the document's project.

Use `source-sentry -C <path> check` to run from another directory, as in git.
Document and lockfile arguments are filesystem paths from that effective
starting directory, or native absolute paths. Relative source arguments
start there too; in declarations they are relative to the document. In all
source spellings, `/` means the document's project root and `:/` means the
repository root. Sources may leave their project but never the repository.
JSON subjects and diagnostic locations carry `project` (`.` for the
repository project), with paths relative to that project. A JSON `attest`
command runs from its document's project root.

A source counts as changed when its content does, as git would store it:
git's own conversions, such as line endings, apply. Files in Git LFS are
compared by their pointers, which Source Sentry computes without running
LFS. A file behind any other clean filter, and a submodule, cannot be
compared, and is reported as an error. Repositories with SHA-1 and SHA-256
object names are both supported.

[`docs/how-it-works.md`](docs/how-it-works.md) explains what Source Sentry
does and why. [`docs/cli.md`](docs/cli.md) describes every command, and
[`structure/`](structure/README.md) shows every file format with examples.
[`recipes/`](recipes/README.md) holds CI recipes for GitHub Actions, GitLab
CI, Forgejo Actions and Woodpecker.

[`skills/source-sentry/`](skills/source-sentry/SKILL.md) is an
[Agent Skill](https://agentskills.io) that teaches coding agents to keep
documentation current with Source Sentry's commands rather than by editing
its files. Copy the folder into the skills directory of your agent.

## Workspace

`Cargo.toml` is the workspace manifest. It centralizes package metadata,
dependency versions and lint policy for its three members, in `crates/`:

- `crates/core/` (`source-sentry-core`): the library. It performs no I/O.
- `crates/git/` (`source-sentry-git`): the git backend, which implements the
  core's traits.
- `crates/cli/` (`source-sentry`): the executable.

The members share one `Cargo.lock` and `target/` directory.

## Building and testing

Run these from this directory:

```sh
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets --features source-sentry-core/testing -- -D warnings
cargo test --locked --workspace --features source-sentry-core/testing
RUSTDOCFLAGS="-D warnings" cargo doc --locked --workspace --no-deps
```

The `testing` feature exposes the core's in-memory fakes to the other
crates' tests. `rust-toolchain.toml` pins the Rust version.

## Dependencies

Each dependency in `Cargo.toml` carries a comment naming the job it serves.
Remove a dependency together with its comment once it proves unnecessary.

## Contributing

Source Sentry's home is <https://git.gabmacn.com/gmn/source-sentry>, where
issues are tracked; sign in there with a GitHub account.
<https://github.com/gabmacn/source-sentry> is a mirror of it, which also
accepts pull requests. See [`CONTRIBUTING.md`](CONTRIBUTING.md).

## Licence

MIT; see [`LICENSE`](LICENSE).
