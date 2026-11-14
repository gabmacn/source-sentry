#!/bin/sh
# Installs Source Sentry on Linux or macOS from a release archive.
#
#   curl -fsSL https://git.gabmacn.com/gmn/source-sentry/raw/branch/main/install/install.sh | sh
#   curl -fsSL .../install.sh | sh -s -- --version 0.1.0-rc.2 --dir ~/bin
#
# It downloads the archive for this machine and the release's SHA256SUMS,
# refuses an archive whose checksum does not match, checks that the binary
# runs, and copies it into the install directory. It never edits shell
# configuration: if the directory is not on PATH, it prints the line to add.
#
# Options:
#   --version <version>  The release to install, such as 0.1.0. By default,
#                        the latest release that is not a release candidate.
#   --dir <directory>    Where to put the binary. By default
#                        $SOURCE_SENTRY_INSTALL_DIR, or else ~/.local/bin.
#   -h, --help           Print this help.
#
# Environment:
#   SOURCE_SENTRY_INSTALL_DIR  The default for --dir.
#   SOURCE_SENTRY_RELEASES     The releases page to download from. By default
#                              https://git.gabmacn.com/gmn/source-sentry/releases;
#                              https://github.com/gabmacn/source-sentry/releases
#                              holds the same files.
#
# Windows uses install.ps1 instead.

set -eu

releases="${SOURCE_SENTRY_RELEASES:-https://git.gabmacn.com/gmn/source-sentry/releases}"
releases="${releases%/}"
version=""
dir="${SOURCE_SENTRY_INSTALL_DIR:-${HOME:?HOME is not set}/.local/bin}"

say() {
	printf '%s\n' "$*"
}

fail() {
	printf 'install.sh: %s\n' "$*" >&2
	exit 1
}

usage() {
	cat <<'EOF'
Installs Source Sentry on Linux or macOS from a release archive.

Usage: install.sh [--version <version>] [--dir <directory>]

  --version <version>  The release to install, such as 0.1.0. By default, the
                       latest release that is not a release candidate.
  --dir <directory>    Where to put the binary. By default
                       $SOURCE_SENTRY_INSTALL_DIR, or else ~/.local/bin.

SOURCE_SENTRY_RELEASES names the releases page to download from.
EOF
}

while [ $# -gt 0 ]; do
	case "$1" in
	--version)
		[ $# -ge 2 ] || fail "--version needs a version, such as 0.1.0"
		version="$2"
		shift 2
		;;
	--version=*)
		version="${1#--version=}"
		shift
		;;
	--dir)
		[ $# -ge 2 ] || fail "--dir needs a directory"
		dir="$2"
		shift 2
		;;
	--dir=*)
		dir="${1#--dir=}"
		shift
		;;
	-h | --help)
		usage
		exit 0
		;;
	*)
		fail "unknown option '$1'; see --help"
		;;
	esac
done

# A version is given without the tag's `v`.
version="${version#v}"
case "$version" in
*/* | *' '*) fail "'$version' is not a version" ;;
esac

# Downloads: curl, or else wget.
if command -v curl >/dev/null 2>&1; then
	fetch() {
		curl --proto '=https' --tlsv1.2 --fail --silent --show-error --location --output "$2" "$1"
	}
	# Prints the URL a redirect chain ends at.
	final_url() {
		curl --proto '=https' --tlsv1.2 --fail --silent --show-error --location \
			--output /dev/null --write-out '%{url_effective}' "$1"
	}
elif command -v wget >/dev/null 2>&1; then
	fetch() {
		wget --quiet --https-only --output-document "$2" "$1"
	}
	final_url() {
		location="$(wget --https-only --spider --server-response "$1" 2>&1 |
			sed -n 's/^ *[Ll]ocation: *//p' | tail -n 1 | tr -d '\r')"
		[ -n "$location" ] && printf '%s\n' "$location"
	}
else
	fail "neither curl nor wget is installed"
fi

# The SHA-256 of a file, as lowercase hexadecimal.
if command -v sha256sum >/dev/null 2>&1; then
	sha256() { sha256sum "$1" | cut -d ' ' -f 1; }
elif command -v shasum >/dev/null 2>&1; then
	sha256() { shasum -a 256 "$1" | cut -d ' ' -f 1; }
elif command -v openssl >/dev/null 2>&1; then
	sha256() { openssl dgst -sha256 -r "$1" | cut -d ' ' -f 1; }
else
	fail "no SHA-256 tool found (sha256sum, shasum or openssl)"
fi

# The target triple of this machine.
os="$(uname -s)"
arch="$(uname -m)"
case "$arch" in
x86_64 | amd64) arch=x86_64 ;;
aarch64 | arm64) arch=aarch64 ;;
*) fail "no release is built for the $arch architecture; build from source with cargo instead" ;;
esac
case "$os" in
Linux)
	# The Linux binaries link against glibc; musl systems such as Alpine
	# cannot run them.
	if ldd --version 2>&1 | grep -qi musl; then
		fail "no release is built for musl-based Linux; build from source with cargo instead"
	fi
	target="$arch-unknown-linux-gnu"
	;;
Darwin)
	# Under Rosetta, uname reports x86_64 on Apple silicon.
	if [ "$arch" = x86_64 ] && [ "$(sysctl -n hw.optional.arm64 2>/dev/null || true)" = 1 ]; then
		arch=aarch64
	fi
	[ "$arch" = aarch64 ] || fail "only Apple silicon Macs have a release build; build from source with cargo instead"
	target=aarch64-apple-darwin
	;;
*)
	fail "no release is built for $os; on Windows, use install.ps1"
	;;
esac

# The latest stable release, through the releases page's redirect. Both
# forges skip release candidates there.
if [ -z "$version" ]; then
	latest="$(final_url "$releases/latest" 2>/dev/null || true)"
	case "$latest" in
	*/tag/v*) version="${latest##*/tag/v}" ;;
	*)
		fail "found no stable release at $releases; choose one with --version, such as 0.1.0-rc.2"
		;;
	esac
fi

name="source-sentry-$version-$target"
archive="$name.tar.gz"
base="$releases/download/v$version"

work="$(mktemp -d 2>/dev/null || mktemp -d -t source-sentry)"
trap 'rm -rf "$work"' EXIT
trap 'exit 130' INT TERM

say "Downloading Source Sentry $version for $target."
fetch "$base/SHA256SUMS" "$work/SHA256SUMS" ||
	fail "could not download $base/SHA256SUMS; is $version a released version?"
fetch "$base/$archive" "$work/$archive" ||
	fail "could not download $base/$archive; the release may not include $target yet"

# Lines are `<hash>  <name>`, or `<hash> *<name>` in binary mode.
expected="$(awk -v name="$archive" '{ file = $2; sub(/^\*/, "", file) } file == name { print $1 }' "$work/SHA256SUMS")"
[ -n "$expected" ] || fail "SHA256SUMS does not list $archive"
actual="$(sha256 "$work/$archive")"
[ "$actual" = "$expected" ] ||
	fail "the checksum of $archive does not match SHA256SUMS; nothing was installed"
say "Checksum verified."

tar -xzf "$work/$archive" -C "$work"
binary="$work/$name/source-sentry"
[ -f "$binary" ] || fail "$archive does not contain $name/source-sentry"
reported="$("$binary" --version 2>&1)" ||
	fail "the binary does not run on this machine: $reported"
[ "$reported" = "source-sentry $version" ] ||
	fail "the binary reports '$reported', not 'source-sentry $version'"

mkdir -p "$dir"
# Copied beside its destination first, so that a failed copy never leaves a
# partial binary in place.
cp "$binary" "$dir/.source-sentry.tmp"
chmod 0755 "$dir/.source-sentry.tmp"
mv -f "$dir/.source-sentry.tmp" "$dir/source-sentry"
say "Installed $reported to $dir/source-sentry."

case ":$PATH:" in
*":$dir:"*) ;;
*)
	say ""
	say "$dir is not on your PATH. Add it in your shell's startup file:"
	say "  sh, bash, zsh:  export PATH=\"$dir:\$PATH\""
	say "  fish:           fish_add_path $dir"
	;;
esac
say ""
say "Shell completions: source-sentry completions --help"
