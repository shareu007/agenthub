# Installation

AgentHub installs two equivalent commands: `ah` and `agenthub`.
Supported targets are macOS 13+ and 64-bit Linux, on ARM64 and x86_64.

## Online installer

```sh
curl -fsSL https://raw.githubusercontent.com/shareu007/agenthub/main/install.sh -o install.sh
sh install.sh
```

This needs a published [Release](https://github.com/shareu007/agenthub/releases).
There is no published release yet. Source installation is available immediately;
separately supplied cross-build previews should be used according to their
verification status, not assumed to be tested native releases.

Options:

```sh
sh install.sh --version v0.1.0
sh install.sh --bin-dir "$HOME/.local/bin"
sh install.sh --from-source
```

The default destination is `~/.local/bin`; add it to PATH if necessary:

```sh
export PATH="$HOME/.local/bin:$PATH"
```

## Offline installation

On a connected machine, download the archive for the **destination machine** and
its matching `.sha256` file. Copy both onto the offline machine. No Rust, Git,
network connection or separate SQLite installation is required there.

| Destination | Archive |
| --- | --- |
| Linux, Intel / AMD x86_64 | `agenthub-x86_64-unknown-linux-musl.tar.gz` |
| Linux, ARM64 | `agenthub-aarch64-unknown-linux-musl.tar.gz` |
| macOS, Intel | `agenthub-x86_64-apple-darwin.tar.gz` |
| macOS, Apple Silicon | `agenthub-aarch64-apple-darwin.tar.gz` |

Each archive includes `ah`, `agenthub`, `install.sh`, `BUILD-INFO.json` and licenses.
Release downloads also include a per-archive `.sha256` and a combined `SHA256SUMS`.
Do not rename the archive: the installer uses its name to check the target platform.

### Linux example

Run in the directory containing the copied archive and checksum:

```sh
sha256sum -c agenthub-x86_64-unknown-linux-musl.tar.gz.sha256
mkdir -p agenthub-install
tar -xzf agenthub-x86_64-unknown-linux-musl.tar.gz -C agenthub-install
sh agenthub-install/install.sh --archive ./agenthub-x86_64-unknown-linux-musl.tar.gz
```

For ARM64 Linux, replace `x86_64` with `aarch64` in the commands.

### Apple Silicon example

```sh
shasum -a 256 -c agenthub-aarch64-apple-darwin.tar.gz.sha256
mkdir -p agenthub-install
tar -xzf agenthub-aarch64-apple-darwin.tar.gz -C agenthub-install
sh agenthub-install/install.sh --archive ./agenthub-aarch64-apple-darwin.tar.gz
```

For an Intel Mac, replace `aarch64` with `x86_64`.
`--archive` verifies the checksum again and never downloads or builds anything.
It accepts an adjacent per-archive `.sha256` or the combined `SHA256SUMS`.
Add `--bin-dir /your/directory` to choose a different writable destination.

You can alternatively copy the two executables into a directory on PATH yourself,
after verifying the archive. No agent data is bundled: indexing uses the sessions
already present on the destination machine. Native agents must be installed to
resume sessions; offline models/authentication are controlled by those agents.

## Source installation

Requires Git, Rust 1.85+ and a C compiler (Xcode Command Line Tools on macOS,
or your Linux distribution's C build toolchain).

```sh
git clone https://github.com/shareu007/agenthub.git
cd agenthub
cargo install --locked --path crates/cli --force
```

Cargo installs both commands into `~/.cargo/bin`.

## Verification and updates

```sh
ah --version
ah doctor
ah sync
ah
```

An offline update is the same operation using a newer archive and its checksum;
existing session/index data is retained. `BUILD-INFO.json` records the target,
source revision, and whether packaging ran the binaries' version check natively.
A cross-build with `native_version_check: false` needs validation on the destination
OS; a successful cross-link does not establish runtime compatibility.

For script/binary installs, uninstall by removing the two installed command files.
For Cargo installs, use `cargo uninstall agenthub-cli`.
