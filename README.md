# Source Sentry

Tracks documentation and records against the repository sources they describe.
It supports documentation roots per product, and distinguishes documentation,
which must follow its sources, from records, which must not change once
written.

When a source changes, the documents that describe it become stale, and each
document gets a deadline set by its schedule. A context decides what staleness
means for a build: a privacy policy can block a production deploy while a
marketing page only warns.

## Installing

The first release has not been published yet. Once it has, there are two ways
to install Source Sentry:

- **Prebuilt binaries** for Linux (x86_64 and aarch64), macOS (Apple silicon)
  and Windows (x86_64), attached to each release at
  <https://git.gabmacn.com/gmn/source-sentry/releases>. Each comes in an
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

  Then install, with `--locked` so the dependency versions that were tested
  are the ones used:

  ```sh
  cargo install --locked --registry gmn source-sentry
  ```

Source Sentry runs `git` for some operations, so git must be on the `PATH`.
[`CHANGELOG.md`](CHANGELOG.md) lists the changes in each release.

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

[`docs/cli.md`](docs/cli.md) describes every command, and
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

See [`CONTRIBUTING.md`](CONTRIBUTING.md).

## Licence

MIT; see [`LICENSE`](LICENSE).
