# Packaging and releases

The public release artifacts are `ah` and `agenthub`, packaged together with the
MIT license and third-party notices. There is no package registry dependency at
install time.

## Platform matrix

| Archive target | Build/test runner | Users |
| --- | --- | --- |
| `x86_64-unknown-linux-musl` | Ubuntu 24.04 x86_64 | 64-bit Intel/AMD Linux |
| `aarch64-unknown-linux-musl` | Ubuntu 24.04 ARM64 | 64-bit ARM Linux |
| `x86_64-apple-darwin` | macOS 15 Intel | Intel Mac, macOS 13+ |
| `aarch64-apple-darwin` | macOS 15 Apple Silicon | Apple Silicon Mac, macOS 13+ |

Linux builds use bundled SQLite and musl, with no dynamic interpreter. macOS
builds set `MACOSX_DEPLOYMENT_TARGET=13.0`. Each release runner tests and executes
its own target; cross-compilation alone is not treated as a runtime check.
The packager checks the binary format, architecture, static Linux linking and
version before creating an archive.

The script/format tests simulate all platform selections. That verifies installer
logic, not execution on another OS/architecture. Only completed matrix jobs prove
that the corresponding release artifact was built and tested.

## Prepare the first release

1. Review the English/Chinese README and example configuration. The current README
   labels source installation as available, and binary installation as pending.
2. Update `[workspace.package].version` in `Cargo.toml` and regenerate `Cargo.lock`
   when changing version. It currently starts at `0.1.0`.
3. Run the local checks and commit the intended release revision:

   ```sh
   cargo test --locked --workspace
   cargo clippy --locked --workspace --all-targets -- -D warnings
   cargo build --locked --release --workspace
   python3 scripts/verify_installer.py
   python3 scripts/verify_sql.py
   python3 scripts/verify_fixtures.py
   python3 scripts/verify_selector.py --bin-dir target/release
   ```

4. When ready, create and push the matching tag:

   ```sh
   git tag -a v0.1.0 -m "AgentHub v0.1.0"
   git push origin v0.1.0
   ```

5. The **Release** workflow builds/tests all four targets, then creates a **draft**
   GitHub Release with four archives and `SHA256SUMS`. It does not publish it.
   A missing platform or failed check prevents the draft step. You can also run
   the workflow manually against an existing version tag.
6. Review the draft, release notes and all four assets. Test the installer after
   publishing; then remove the “after the first release” label from both READMEs.
   Publishing the draft and making the repository public are separate owner actions.

Do not create a new version tag with a version that differs from `Cargo.toml`.
The workflow refuses a mismatch and refuses to silently replace an existing
release. If a draft already exists, manage it explicitly before rerunning the
creation step.

Native release builds use Linux ARM runners intended for a public repository.
No Homebrew formula, crates.io package, signed Apple distribution, or Windows
release is advertised. macOS archives are not notarized; never suggest disabling
Gatekeeper or system security settings to run a download.

## Installer contract

`install.sh` uses HTTPS GitHub release downloads, follows redirects only to HTTPS,
and verifies the selected archive against `SHA256SUMS`. It extracts only `ah` and
`agenthub`, checks that both are regular files, stages both, then replaces the
commands. Checksums detect download corruption; they are not publisher signatures.
A failed download/build/checksum leaves existing commands alone. Each final rename
is atomic; replacing the pair is not a filesystem-wide transaction.

It supports `--version v0.1.0`, `--bin-dir /path`, `AGENTHUB_INSTALL_DIR`, and
`--from-source`. Source mode builds main by default, or the selected release tag,
using `cargo install --locked`. No fallback silently installs a Rust toolchain.
No shell startup files or agent stores are modified.

For a custom or system-wide installation, choose a directory you own or install
reviewed executables manually. Avoid running a remote installer with sudo.
To uninstall a script/binary installation, remove the `ah` and `agenthub` files
from the chosen install directory. This does not delete the local index or
original sessions. For a Cargo installation, use `cargo uninstall agenthub-cli`.

## Re-record the demo

The GIF and still image are captured from the real CLI using disposable,
non-private sample sessions. The script never launches an agent or model. Sample
Claude/Codex/Pi transcripts are synthetic; OpenCode/AGY start from the checked-in
controlled fixtures. Agent executable stubs only make installation detection
available to this non-resuming demo.

On a Linux development machine with DejaVu Sans Mono (or pass `--font`):

```sh
python3 -m venv .demo-venv
.demo-venv/bin/pip install Pillow pyte==0.8.2
.demo-venv/bin/python scripts/record_demo.py --binary target/release/ah
```

This writes `docs/assets/demo.gif` and `docs/assets/demo.png`. Review every frame
before committing. The recording uses a temporary config and storage paths;
it never reads the default AgentHub index or user's native session stores.
