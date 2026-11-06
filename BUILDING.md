# BUILDING.md — generating the executable for each OS

There is **one Cargo project**. Every operating system builds the same source into a different
executable; there is no per-OS crate to maintain.

- **Native build on the target OS is the recommended path.** It guarantees the right C toolchain,
  SDK, and runtime library versions.
- **Cross-compilation is optional.** Linux targets cross-build well; macOS targets practically
  require a Mac or CI because they need Apple's SDK.
- The release profile already enables LTO, strips symbols, and targets one codegen unit, so
  `--release` builds the deployment binary.

The project requires **Rust 1.99 or newer** (`rust-version = "1.99"` in `Cargo.toml`) and a C
toolchain, because `ring` (pulled in by `rustls`) compiles C and assembly. TLS uses `rustls`, so no
OpenSSL development package is needed.

## Targets at a glance

| Platform | Rust target | Artifact |
|---|---|---|
| Windows x64 | `x86_64-pc-windows-msvc` | `mcp-ollama-web-rust.exe` |
| Linux x64, glibc | `x86_64-unknown-linux-gnu` | `mcp-ollama-web-rust` |
| Linux x64, static | `x86_64-unknown-linux-musl` | `mcp-ollama-web-rust` |
| Linux arm64 | `aarch64-unknown-linux-gnu` | `mcp-ollama-web-rust` |
| macOS Apple Silicon | `aarch64-apple-darwin` | `mcp-ollama-web-rust` |
| macOS Intel | `x86_64-apple-darwin` | `mcp-ollama-web-rust` |

Check what a machine already has:

```sh
rustc --version
cargo --version
rustup target list --installed
```

## Native builds

### Windows x64

Install the **Visual Studio Build Tools** with the "Desktop development with C++" workload, then:

```powershell
rustup target add x86_64-pc-windows-msvc
cargo build --release --target x86_64-pc-windows-msvc
```

Output: `target\x86_64-pc-windows-msvc\release\mcp-ollama-web-rust.exe`.

A plain `cargo build --release` on a Windows x64 host produces the same binary under
`target\release\`. `dist\` currently contains this build.

### Linux x64

Install a C toolchain (Debian/Ubuntu shown):

```sh
sudo apt-get update
sudo apt-get install -y build-essential
rustup target add x86_64-unknown-linux-gnu
cargo build --release --target x86_64-unknown-linux-gnu
```

Output: `target/x86_64-unknown-linux-gnu/release/mcp-ollama-web-rust`.

For a binary that runs across distributions without depending on the build machine's glibc, prefer
musl:

```sh
sudo apt-get install -y musl-tools
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl
```

A musl build is statically linked; `ldd` reports "not a dynamic executable". If `ring` complains
during a musl build, use `cross` (below) rather than assembling the toolchain by hand.

### Linux arm64

On an arm64 host, build natively:

```sh
rustup target add aarch64-unknown-linux-gnu
cargo build --release --target aarch64-unknown-linux-gnu
```

On an x64 host, use `cross` or a CI arm64 runner (see below).

### macOS

Install the Xcode Command Line Tools first, then build for the host architecture:

```sh
xcode-select --install
cargo build --release
```

Output: `target/release/mcp-ollama-web-rust`.

Build both architectures and create a universal binary:

```sh
rustup target add aarch64-apple-darwin x86_64-apple-darwin
cargo build --release --target aarch64-apple-darwin
cargo build --release --target x86_64-apple-darwin

lipo -create -output mcp-ollama-web-rust \
  target/aarch64-apple-darwin/release/mcp-ollama-web-rust \
  target/x86_64-apple-darwin/release/mcp-ollama-web-rust
```

`lipo` comes with the Xcode Command Line Tools.

## Cross-compilation

`cross` runs builds inside Docker images with the target linker and libc already installed. It is
the least fragile way to cross-build Linux targets from Windows or macOS.

```sh
cargo install cross --git https://github.com/cross-rs/cross
rustup target add x86_64-unknown-linux-gnu x86_64-unknown-linux-musl aarch64-unknown-linux-gnu

cross build --release --target x86_64-unknown-linux-gnu
cross build --release --target x86_64-unknown-linux-musl
cross build --release --target aarch64-unknown-linux-gnu
```

Prerequisites: Docker Desktop (Windows/macOS) or Docker Engine (Linux) running.

From Windows without `cross`, WSL is the simplest route:

```powershell
wsl --install
```

Then follow the Linux native instructions inside WSL. The resulting binary is a Linux executable;
copy it out with `cp` into `/mnt/c/...`.

**macOS targets cannot be cross-built from Windows/Linux** in practice: Apple's SDK is required and
its licence does not permit redistributing it with the toolchain. Build on a Mac or use a hosted
macOS runner.

### Cross-building Windows from Linux/macOS (optional)

```sh
rustup target add x86_64-pc-windows-gnu
# Debian/Ubuntu:
sudo apt-get install -y mingw-w64
cargo build --release --target x86_64-pc-windows-gnu
```

If the linker is not found, add this to `.cargo/config.toml`:

```toml
[target.x86_64-pc-windows-gnu]
linker = "x86_64-w64-mingw32-gcc"
```

CI is usually a better choice than maintaining a local cross toolchain.

## CI matrix

A GitHub Actions matrix is the most reliable way to produce all platform binaries. Each job builds
on its own OS, so no cross-compilation is involved except the optional macOS Intel target.

```yaml
name: build-binaries

on:
  workflow_dispatch:
  push:
    tags: ["v*"]

jobs:
  build:
    strategy:
      fail-fast: false
      matrix:
        include:
          - os: windows-latest
            target: x86_64-pc-windows-msvc
            artifact: mcp-ollama-web-rust.exe
            bundle: windows-x64
          - os: ubuntu-latest
            target: x86_64-unknown-linux-musl
            artifact: mcp-ollama-web-rust
            bundle: linux-x64
          - os: macos-latest
            target: aarch64-apple-darwin
            artifact: mcp-ollama-web-rust
            bundle: macos-arm64
          - os: macos-latest
            target: x86_64-apple-darwin
            artifact: mcp-ollama-web-rust
            bundle: macos-x64
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
      - name: Install Rust and target
        run: |
          rustup toolchain install 1.99.0 --profile minimal
          rustup default 1.99.0
          rustup target add ${{ matrix.target }}
      - name: Install musl tools
        if: matrix.target == 'x86_64-unknown-linux-musl'
        run: |
          sudo apt-get update
          sudo apt-get install -y musl-tools
      - name: Build
        run: cargo build --release --target ${{ matrix.target }}
      - name: Upload artifact
        uses: actions/upload-artifact@v4
        with:
          name: ${{ matrix.bundle }}
          path: target/${{ matrix.target }}/release/${{ matrix.artifact }}
          if-no-files-found: error
```

Add another row for Linux arm64 only if you have an arm64 runner; otherwise cross-build it with
`cross` in a separate job.

## Package a platform bundle

Keep each platform in its own directory so the executables and guides cannot be confused:

```text
dist/
├── windows-x64/
├── linux-x64/
└── macos-arm64/
```

The current `dist/` is the Windows x64 bundle at the top level. When you add more platforms, move it
to `dist/windows-x64/` and keep every `INSTALL.md` specific to its platform.

### Linux/macOS

```sh
VERSION=0.5.0
TARGET=x86_64-unknown-linux-musl        # or aarch64-apple-darwin, x86_64-apple-darwin, ...
PLATFORM=linux-x64                      # or macos-arm64, macos-x64, ...
BUNDLE="dist/$PLATFORM"

mkdir -p "$BUNDLE"
cp "target/$TARGET/release/mcp-ollama-web-rust" "$BUNDLE/"
cp credentials.example LICENSE "$BUNDLE/"
chmod +x "$BUNDLE/mcp-ollama-web-rust"

# Linux:
( cd "$BUNDLE" && sha256sum mcp-ollama-web-rust credentials.example LICENSE > SHA256SUMS.txt )
# macOS (no sha256sum by default):
# ( cd "$BUNDLE" && shasum -a 256 mcp-ollama-web-rust credentials.example LICENSE > SHA256SUMS.txt )

tar -czf "dist/mcp-ollama-web-rust-${VERSION}-${PLATFORM}.tar.gz" -C dist "$PLATFORM"
```

Add a platform-specific `INSTALL.md` and, if you want an installer, an `install.sh`. The Windows
`install.ps1` is not portable; a Unix installer is a short script that copies the binary to
`~/.local/bin`, creates `~/.config/mcp-ollama-web-rust/.credentials` (`chmod 600`), and prints the
MCP client entry.

### Windows

```powershell
$Version = "0.5.0"
$Bundle  = "dist\windows-x64"
$Target  = "x86_64-pc-windows-msvc"

New-Item -ItemType Directory -Force $Bundle | Out-Null
Copy-Item "target\$Target\release\mcp-ollama-web-rust.exe" $Bundle
Copy-Item credentials.example, LICENSE $Bundle
Copy-Item dist\INSTALL.md $Bundle

Get-FileHash "$Bundle\mcp-ollama-web-rust.exe" -Algorithm SHA256 |
  ForEach-Object { "$($_.Hash.ToLower())  mcp-ollama-web-rust.exe" } |
  Set-Content "$Bundle\SHA256SUMS.txt"

Compress-Archive -Path $Bundle -DestinationPath "dist\mcp-ollama-web-rust-$Version-windows-x64.zip" -Force
```

Never copy `.credentials` into a bundle. Ship `credentials.example` only; the installer or the user
creates the real file on the target machine.

## Verify a binary

```sh
./mcp-ollama-web-rust --version
printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25"}}' \
  | ./mcp-ollama-web-rust
```

Expected: the version, then a JSON-RPC result containing `"serverInfo"` and
`"protocolVersion":"2025-11-25"`.

Check the file format when cross-building:

| OS | Command | Expected |
|---|---|---|
| Linux | `file mcp-ollama-web-rust` | `ELF 64-bit ... executable` |
| macOS | `file mcp-ollama-web-rust` | `Mach-O 64-bit executable arm64` or `x86_64` |
| Windows | `file mcp-ollama-web-rust.exe` (Git Bash) | `PE32+ executable ... x86-64` |

Run the test suite on each target OS or in CI:

```sh
cargo test --all-targets
```

A successful compile is not a runtime guarantee. At minimum, run the handshake and one
`tools/call` on the target OS (or in a matching CI job).

When the crate version changes, update the `**Version:**` line in each platform's `INSTALL.md`.
`tests/dist_bundle.rs` fails if `dist/INSTALL.md` does not match `CARGO_PKG_VERSION`.

## Platform notes

- **Linux glibc floor.** A glibc build runs only on systems with a glibc at least as new as the build
  machine's. Build on the oldest distribution you support, or ship the musl binary.
- **macOS Gatekeeper.** A downloaded unsigned binary may be quarantined. Verify its SHA-256 first,
  then remove the quarantine attribute if your policy allows it:
  `xattr -d com.apple.quarantine mcp-ollama-web-rust`. For wider distribution, sign and notarize
  the binary.
- **Windows SmartScreen.** An unsigned executable may warn on first run. Code-sign it for enterprise
  distribution.
- **Architecture mismatch.** `bad CPU type in executable` on macOS means an Intel binary on Apple
  Silicon (or the reverse); use the matching target or the universal binary.
- **Proxy variables are portable.** `HTTP_PROXY`, `HTTPS_PROXY`, and `NO_PROXY` work the same on all
  three operating systems and are read directly by the HTTP client.
- **Credentials paths are OS-specific.** Use an absolute path in `OLLAMA_CREDENTIALS_FILE`, for
  example `C:/Users/<user>/...` on Windows, `/Users/<user>/...` on macOS, and
  `/home/<user>/...` on Linux.
