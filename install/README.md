# Installing Source Sentry

Every method here installs the same prebuilt binaries: those attached to the
releases at <https://git.gabmacn.com/gmn/source-sentry/releases>, which are
copied with the same files to
<https://github.com/gabmacn/source-sentry/releases>. Binaries exist for
Linux (x86_64 and aarch64, with glibc), macOS (Apple silicon) and Windows
(x86_64). Elsewhere, build from source with `cargo install`, as the
[README](../README.md#installing) describes.

Versions before 0.1.0 are release candidates, such as `0.1.0-rc.3`. No
method installs a release candidate unless it is named.

| Method | Suits | Verifies the archive against `SHA256SUMS` |
|---|---|---|
| [Install scripts](#install-scripts) | One machine, by hand | Yes |
| [cargo-binstall](#cargo-binstall) | Rust developers who already use it | No; HTTPS only |
| [proto](#proto) | Pinning one version per repository | Yes |
| [moon](#moon) | Monorepos run with moon, through proto | Yes |

## Install scripts

`install.sh`, for Linux and macOS, and `install.ps1`, for Windows, download
the archive for the machine and the release's `SHA256SUMS`, refuse an archive
whose checksum does not match, check that the binary runs and reports the
expected version, and then copy it into the install directory. They never
change `PATH` or any shell configuration: when the directory is not on
`PATH`, they print how to add it.

Without a version, they install the latest release that is not a release
candidate. Until 0.1.0 is released there is none, so name a version.

### Linux and macOS

```sh
curl -fsSL https://git.gabmacn.com/gmn/source-sentry/raw/branch/main/install/install.sh | sh -s -- --version 0.1.0-rc.3
```

The binary goes to `~/.local/bin`, or to the directory given with `--dir` or
in `SOURCE_SENTRY_INSTALL_DIR`. The script needs `curl` or `wget`, `tar`, and
`sha256sum`, `shasum` or `openssl`. It refuses musl-based Linux, such as
Alpine, and Intel Macs, for which no binary is built. A binary that `curl`
downloads is not quarantined by macOS, so it runs without `xattr`.

### Windows

```powershell
& ([scriptblock]::Create((irm https://git.gabmacn.com/gmn/source-sentry/raw/branch/main/install/install.ps1))) -Version 0.1.0-rc.3
```

Once 0.1.0 is released, `irm <url> | iex` installs the latest release. The
binary goes to `%LOCALAPPDATA%\Programs\source-sentry`, or to the directory
given with `-Dir` or in `SOURCE_SENTRY_INSTALL_DIR`; `SOURCE_SENTRY_VERSION`
can name the version instead of `-Version`. The script runs in Windows
PowerShell 5.1 and later. On Windows 11 on Arm it installs the x86_64
binary, which Windows runs through emulation.

### Both

`SOURCE_SENTRY_RELEASES` names the releases page to download from, such as
`https://github.com/gabmacn/source-sentry/releases`. To read a script before
running it, download it first:

```sh
curl -fsSLO https://git.gabmacn.com/gmn/source-sentry/raw/branch/main/install/install.sh
sh install.sh --help
```

To uninstall, delete the binary.

## cargo-binstall

[cargo-binstall](https://github.com/cargo-bins/cargo-binstall) installs the
release binary in place of compiling the crate, from the metadata in the
`source-sentry` crate. With the `gmn` registry configured as the
[README](../README.md#installing) shows:

```sh
cargo binstall --registry gmn --version <version> source-sentry
```

It works from 0.1.0-rc.3, the first version published with the metadata. cargo-binstall does not check `SHA256SUMS`; it relies on HTTPS.

## proto

[proto](https://moonrepo.dev/proto) installs a version per repository,
pinned in the repository's `.prototools`, through the plugin
[`proto-plugin.toml`](proto-plugin.toml):

```toml
source-sentry = "0.1.0-rc.3"

[plugins.tools]
source-sentry = "https://git.gabmacn.com/gmn/source-sentry/raw/branch/main/install/proto-plugin.toml"
```

proto loads a plugin only from a `.prototools` that each person has trusted,
so after reviewing the file, run `proto trust .prototools` once. Then
`proto install` installs every pinned tool, and `source-sentry`, through
proto's shims, or `proto run source-sentry -- check` runs the pinned version.
proto checks each archive against the release's `SHA256SUMS`, and reads the
available versions from the repository's tags.

## moon

[`moon.yml`](moon.yml) is a moon project that runs Source Sentry through the
proto plugin, so that a team installs nothing by hand:

1. Pin Source Sentry in the workspace's `.prototools`, as above.
2. Copy `moon.yml` to `tools/source-sentry/moon.yml`, and add the project to
   `.moon/workspace.yml`:

   ```yaml
   projects:
     source-sentry: 'tools/source-sentry'
   ```

3. Run its tasks:

   ```sh
   moon run source-sentry:check    # every document, failing on a warning
   moon run source-sentry:review   # review pending documents, on a terminal
   ```

Both tasks depend on `install`, which installs the pinned version when it is
missing: moon turns proto's own automatic installation off for the processes
it runs. They run from the workspace root, and are never cached, because a
check depends on the repository's history, which moon's inputs cannot
describe. `review` is interactive and does not run in CI; for CI, the
[CI recipes](../recipes/README.md) set up both gates.

## Shell completions

Once installed, `source-sentry completions <shell>` prints the completion
script for bash, zsh, fish, PowerShell or elvish, and
`source-sentry completions --help` shows where each shell expects it.
