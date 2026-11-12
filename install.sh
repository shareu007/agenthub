#!/bin/sh
# Install AgentHub's two commands. Never changes shell profiles or session data.
set -eu

REPO=https://github.com/shareu007/agenthub
version=latest
from_source=false
bin_dir=${AGENTHUB_INSTALL_DIR:-}
work_dir=
stage_dir=

fail() { printf 'agenthub installer: %s\n' "$*" >&2; exit 1; }
usage() {
    cat <<'EOF'
Install agenthub and ah on macOS or Linux (ARM64 / x86_64).

Usage: sh install.sh [--version VERSION] [--bin-dir DIRECTORY] [--from-source]

  --version VERSION  Install a release, e.g. v0.1.0 (default: latest).
  --bin-dir DIR      Install both commands here (default: ~/.local/bin).
  --from-source      Build with Cargo; defaults to main, or the selected version tag.
  -h, --help         Show this help.

Binary installation needs curl, tar and sha256sum or shasum.
Source installation needs Git, Rust 1.85+ and a C compiler.
AGENTHUB_INSTALL_DIR can also set the destination. No sudo required.
EOF
}
while [ "$#" -gt 0 ]; do
    case "$1" in
        --version|--bin-dir)
            if [ "$#" -lt 2 ] || [ -z "$2" ]; then fail "$1 needs a value"; fi
            case "$1" in --version) version=$2;; --bin-dir) bin_dir=$2;; esac
            shift 2 ;;
        --from-source) from_source=true; shift ;;
        -h|--help) usage; exit 0 ;;
        *) fail "unknown argument: $1 (use --help)" ;;
    esac
done
if [ "$version" != latest ]; then
    printf '%s\n' "$version" | grep -Eq '^v?[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z]+([.-][0-9A-Za-z]+)*)?$' || fail 'invalid version; use v0.1.0 or latest'
    case "$version" in v*) ;; *) version=v$version;; esac
fi
case "$(uname -s)" in
    Darwin) platform=apple-darwin ;;
    Linux) platform=unknown-linux-musl ;;
    *) fail 'supported systems: macOS and Linux' ;;
esac
case "$(uname -m)" in
    arm64|aarch64) arch=aarch64 ;;
    x86_64|amd64) arch=x86_64 ;;
    *) fail 'supported architectures: ARM64 and x86_64 (64-bit only)' ;;
esac
target=$arch-$platform
if [ -z "$bin_dir" ]; then
    [ -n "${HOME:-}" ] || fail 'HOME is unset; pass --bin-dir'
    bin_dir=$HOME/.local/bin
fi
case "$bin_dir" in
    /*) ;;
    *) bin_dir=$(pwd -P)/$bin_dir ;;
esac
cleanup() {
    [ -z "$stage_dir" ] || rm -rf "$stage_dir"
    [ -z "$work_dir" ] || rm -rf "$work_dir"
    return 0
}
trap cleanup 0
trap 'exit 130' INT
trap 'exit 143' HUP TERM
work_dir=$(mktemp -d "${TMPDIR:-/tmp}/agenthub-install.XXXXXXXX")
if [ "$from_source" = true ]; then
    for tool in git cargo; do command -v "$tool" >/dev/null 2>&1 || fail "source installation requires $tool"; done
    ref=main
    [ "$version" = latest ] || ref=$version
    printf 'Building AgentHub from %s (requires Rust and a C compiler)...\n' "$ref"
    git clone --quiet --depth 1 --branch "$ref" "$REPO.git" "$work_dir/source"
    cargo install --locked --path "$work_dir/source/crates/cli" --bins --root "$work_dir/build"
    payload=$work_dir/build/bin
else
    for tool in curl tar; do command -v "$tool" >/dev/null 2>&1 || fail "binary installation requires $tool"; done
    if command -v sha256sum >/dev/null 2>&1; then checksum=sha256sum
    elif command -v shasum >/dev/null 2>&1; then checksum=shasum
    else fail 'install sha256sum or shasum to verify the download'; fi
    base=$REPO/releases/latest/download
    [ "$version" = latest ] || base=$REPO/releases/download/$version
    asset=agenthub-$target.tar.gz
    download() {
        curl --proto '=https' --proto-redir '=https' --tlsv1.2 --fail --show-error --silent --location \
            --retry 2 --connect-timeout 15 --max-time 180 "$1" --output "$2"
    }
    printf 'Downloading AgentHub for %s...\n' "$target"
    download "$base/$asset" "$work_dir/$asset" || fail 'release download failed; check Releases, or use --from-source with Rust installed'
    download "$base/SHA256SUMS" "$work_dir/SHA256SUMS" || fail 'could not download release checksums; nothing installed'
    expected=$(awk -v asset="$asset" '$2 == asset {print $1}' "$work_dir/SHA256SUMS")
    [ "${#expected}" -eq 64 ] || fail 'missing or invalid release checksum'
    case "$expected" in *[!0-9a-fA-F]*) fail 'invalid release checksum';; esac
    if [ "$checksum" = sha256sum ]; then
        actual=$(sha256sum "$work_dir/$asset" | awk '{print $1}')
    else
        actual=$(shasum -a 256 "$work_dir/$asset" | awk '{print $1}')
    fi
    [ "$actual" = "$expected" ] || fail 'checksum mismatch; nothing installed'
    payload=$work_dir/unpack
    mkdir "$payload"
    # Extract only the two known members, not arbitrary paths from an archive.
    tar -xzf "$work_dir/$asset" -C "$payload" agenthub ah
fi
for name in agenthub ah; do
    if [ ! -f "$payload/$name" ] || [ -L "$payload/$name" ]; then fail "missing regular binary: $name"; fi
    [ ! -d "$bin_dir/$name" ] || fail "$bin_dir/$name is a directory; choose another --bin-dir"
done
mkdir -p "$bin_dir"
bin_dir=$(cd "$bin_dir" && pwd -P)
# Stage both complete downloads/builds before replacing an existing installation.
stage_dir=$(mktemp -d "$bin_dir/.agenthub-install.XXXXXXXX")
for name in agenthub ah; do
    cp "$payload/$name" "$stage_dir/$name"
    chmod 755 "$stage_dir/$name"
done
for name in agenthub ah; do mv -f "$stage_dir/$name" "$bin_dir/$name"; done
printf 'Installed agenthub and ah in %s\n' "$bin_dir"
case ":${PATH:-}:" in
    *":$bin_dir:"*) printf 'Get started: ah sync, then ah\n' ;;
    *) printf 'Add %s to your PATH, then run: ah sync\n' "$bin_dir" ;;
esac
