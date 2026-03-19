# Unblock — CI/CD Architecture

**Build, test, and distribution strategy for all workspace products.**

| | |
|---|---|
| **Version** | 1.0.0-draft |
| **Author** | Miguel Ramos |
| **Org** | websublime |
| **Repo** | `websublime/unblock` |
| **Date** | March 2026 |
| **Status** | Draft |
| **Depends on** | `unblock-architecture-github.md`, `unblock-desktop-architecture.md` |

---

## Table of Contents

1. [Overview](#1-overview)
2. [Workspace Structure](#2-workspace-structure)
3. [Versioning Strategy](#3-versioning-strategy)
4. [CI — Continuous Integration](#4-ci--continuous-integration)
5. [Release — MCP Server](#5-release--mcp-server)
6. [Release — Desktop App](#6-release--desktop-app)
7. [Distribution Channels](#7-distribution-channels)
8. [Homebrew Tap](#8-homebrew-tap)
9. [npm Wrapper](#9-npm-wrapper)
10. [Secrets and Signing](#10-secrets-and-signing)
11. [Workflow Files Summary](#11-workflow-files-summary)

---

## 1. Overview

The Unblock workspace produces two binaries with fundamentally different build and distribution requirements:

| | MCP Server (`unblock-mcp`) | Desktop App (`unblock-app`) |
|---|---|---|
| **Type** | Headless CLI | GUI application (GPUI) |
| **System deps** | None (static musl on Linux) | Metal (macOS), Vulkan (Linux) |
| **Platforms** | Linux, macOS, Windows | macOS, Linux |
| **Packaging** | Tarballs + installers | `.dmg` (unsigned), AppImage |
| **Release tool** | `cargo-dist` (auto-generated) | Custom GitHub Actions |
| **Channels** | GitHub Releases, Homebrew, npm | GitHub Releases, Homebrew Cask |
| **Version** | Independent (v1.x) | Independent (v2.x) |

### Design Decisions

| # | Decision | Rationale |
|---|---|---|
| CD1 | `cargo-dist` for MCP, custom workflow for desktop | cargo-dist doesn't support `.dmg` signing, notarisation, or AppImage. MCP is its ideal use case |
| CD2 | Independent versioning per product | MCP and desktop have different release cadences. Singular tags avoid coupling |
| CD3 | Unified CI, split release workflows | One `ci.yml` catches cross-crate regressions. Release workflows are product-specific |
| CD4 | `cargo-release` as version orchestrator | Handles version bumps, changelog, and tag creation. Integrates with cargo-dist tag format |
| CD5 | Homebrew tap serves both products | One `websublime/homebrew-tap` repo with formula (MCP) + cask (desktop) |
| CD6 | macOS unsigned distribution initially | No Apple Developer Program subscription upfront. Document Gatekeeper bypass in README. Signing steps commented out in workflow, ready to enable when adoption justifies the investment |

---

## 2. Workspace Structure

```toml
# Cargo.toml (workspace root)
[workspace]
members = [
    "crates/unblock-core",
    "crates/unblock-github",
    "crates/unblock-mcp",
    "crates/unblock-app",
]
resolver = "2"

[workspace.package]
edition = "2024"
license = "MIT"
repository = "https://github.com/websublime/unblock"

[workspace.dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["full"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["json", "env-filter"] }
reqwest = { version = "0.12", features = ["json"] }
petgraph = "0.7"
chrono = { version = "0.4", features = ["serde"] }
snafu = "0.8"
anyhow = "1"
rmcp = { version = "1.0", features = ["server", "transport-io"] }
schemars = "1"
rand = "0.9"

[workspace.lints.clippy]
pedantic = { level = "warn", priority = -1 }
module_name_repetitions = "allow"
missing_errors_doc = "allow"

[workspace.lints.rust]
unsafe_code = "deny"
missing_docs = "warn"
```

Libraries (`unblock-core`, `unblock-github`) do not have independent versions — they are internal. Only the two binaries (`unblock-mcp`, `unblock-app`) are versioned and released.

---

## 3. Versioning Strategy

### 3.1 Independent Versions

Each binary package has its own version in its `Cargo.toml`. Libraries follow the version of whichever binary most recently changed them, but are not published to crates.io.

```
unblock-core     → internal, no published version
unblock-github   → internal, no published version
unblock-mcp      → v1.0.0, v1.1.0, v1.2.0 ...
unblock-app      → v2.0.0, v2.1.0, v2.2.0 ...
```

### 3.2 Tag Format

Tags follow `cargo-dist` Singular Announcement format:

```
unblock-mcp-v1.0.0     → triggers MCP release workflow
unblock-app-v2.0.0     → triggers desktop release workflow
```

### 3.3 cargo-release Configuration

```toml
# Cargo.toml (workspace root)
[workspace.metadata.release]
shared-version = false
tag-name = "{{crate_name}}-v{{version}}"
```

```toml
# crates/unblock-mcp/Cargo.toml
[package.metadata.release]
tag-name = "unblock-mcp-v{{version}}"
```

```toml
# crates/unblock-app/Cargo.toml
[package.metadata.release]
tag-name = "unblock-app-v{{version}}"
```

Release flow:

```bash
# MCP release
cargo release -p unblock-mcp --execute 1.1.0
# → bumps version in Cargo.toml
# → updates CHANGELOG.md
# → commits: "chore: release unblock-mcp v1.1.0"
# → tags: unblock-mcp-v1.1.0
# → pushes commit + tag → triggers cargo-dist workflow

# Desktop release
cargo release -p unblock-app --execute 2.1.0
# → same flow → triggers desktop release workflow
```

---

## 4. CI — Continuous Integration

### 4.1 Workflow: `ci.yml`

Runs on every push and pull request. Split into jobs by dependency weight so that lightweight checks don't wait for heavy builds.

```yaml
# .github/workflows/ci.yml
name: CI
on:
  push:
    branches: [main]
  pull_request:

env:
  CARGO_TERM_COLOR: always
  RUSTFLAGS: "-D warnings"

jobs:
  # ─── Fast checks (< 1 min) ───
  check:
    name: Format + Lint
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt, clippy
      - uses: Swatinem/rust-cache@v2
      - run: cargo fmt --check --all
      - run: cargo clippy --workspace --all-targets -- -D warnings

  # ─── Core + GitHub + MCP tests ───
  test-mcp:
    name: Test MCP (${{ matrix.os }})
    needs: check
    runs-on: ${{ matrix.os }}
    strategy:
      matrix:
        os: [ubuntu-latest, macos-latest]
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - run: cargo test -p unblock-core -p unblock-github -p unblock-mcp

  # ─── Desktop tests (only if desktop code changed) ───
  test-desktop:
    name: Test Desktop (${{ matrix.os }})
    needs: check
    if: |
      github.event_name == 'push' ||
      contains(join(github.event.pull_request.changed_files.*.filename, ' '), 'crates/unblock-app')
    runs-on: ${{ matrix.os }}
    strategy:
      matrix:
        os: [ubuntu-latest, macos-latest]
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - name: Install system deps (Linux)
        if: runner.os == 'Linux'
        run: |
          sudo apt-get update
          sudo apt-get install -y libvulkan-dev libfontconfig-dev libxcb-shape0-dev libxcb-xfixes0-dev
      - run: cargo test -p unblock-app
      - run: cargo build --release -p unblock-app

  # ─── Coverage (MCP only, Linux) ───
  coverage:
    name: Coverage
    needs: test-mcp
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - run: cargo install cargo-tarpaulin
      - run: cargo tarpaulin -p unblock-core -p unblock-github -p unblock-mcp --out xml
      - uses: codecov/codecov-action@v4
        with:
          file: cobertura.xml
```

### 4.2 Why Split Jobs

| Job | Runs when | Duration | Blocks merge |
|---|---|---|---|
| `check` | Always | ~30s | Yes |
| `test-mcp` | Always | ~2min | Yes |
| `test-desktop` | Desktop code changed | ~5min | Yes (if triggered) |
| `coverage` | After MCP tests pass | ~3min | No (informational) |

The desktop build is heavy (GPUI compilation, system deps). Running it on every PR that touches only MCP code is waste. The `if` condition skips it when desktop files are unchanged.

---

## 5. Release — MCP Server

### 5.1 Tool: `cargo-dist`

`cargo-dist` generates the release workflow. Run `cargo dist init` once to bootstrap, then it maintains `.github/workflows/release.yml` automatically.

```toml
# crates/unblock-mcp/Cargo.toml

[package]
name = "unblock-mcp"
version = "1.0.0"

[[bin]]
name = "unblock-mcp"
path = "src/main.rs"

[package.metadata.dist]
# cargo-dist configuration
installers = ["shell", "powershell", "homebrew"]
tap = "websublime/homebrew-tap"
publish-jobs = ["homebrew"]
targets = [
    "x86_64-unknown-linux-musl",
    "aarch64-unknown-linux-musl",
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
    "x86_64-pc-windows-msvc",
]
```

### 5.2 What cargo-dist Generates

Tag push `unblock-mcp-v1.0.0` triggers the auto-generated `release.yml`:

```
Plan    → reads tag, resolves package, generates build plan
Build   → spins up runners per target, builds binaries + tarballs
Host    → creates GitHub Release, uploads artifacts
Publish → updates Homebrew formula in tap repo
Announce → adds release notes from CHANGELOG.md
```

### 5.3 Release Artifacts (per tag)

| Artifact | Platform | Notes |
|---|---|---|
| `unblock-mcp-x86_64-unknown-linux-musl.tar.xz` | Linux x86_64 | Static binary, zero deps |
| `unblock-mcp-aarch64-unknown-linux-musl.tar.xz` | Linux ARM64 | Static binary, zero deps |
| `unblock-mcp-x86_64-apple-darwin.tar.xz` | macOS Intel | |
| `unblock-mcp-aarch64-apple-darwin.tar.xz` | macOS Apple Silicon | |
| `unblock-mcp-x86_64-pc-windows-msvc.zip` | Windows x86_64 | |
| `unblock-mcp-installer.sh` | Unix | Shell installer script |
| `unblock-mcp-installer.ps1` | Windows | PowerShell installer script |

### 5.4 Install Methods

```bash
# Shell installer (Linux/macOS)
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/websublime/unblock/releases/latest/download/unblock-mcp-installer.sh | sh

# Homebrew
brew install websublime/tap/unblock-mcp

# npm
npx @unblock/cli

# Cargo (requires Rust toolchain)
cargo install unblock-mcp
```

---

## 6. Release — Desktop App

### 6.1 Why Not cargo-dist

The desktop app has requirements cargo-dist doesn't handle:

- **`.dmg` creation** — `create-dmg` with custom layout (cargo-dist generates tarballs)
- **AppImage packaging** — `.desktop` file, icon, `AppRun`, `appimagetool`
- **System dependencies** — Vulkan SDK, fontconfig headers on Linux build runners
- **Future: macOS code signing** — `codesign` with Developer ID certificate (when enabled)
- **Future: macOS notarisation** — `xcrun notarytool submit` for Gatekeeper (when enabled)

### 6.2 Workflow: `desktop-release.yml`

```yaml
# .github/workflows/desktop-release.yml
name: Desktop Release
on:
  push:
    tags: ["unblock-app-v*"]

permissions:
  contents: write

env:
  CARGO_TERM_COLOR: always

jobs:
  # ─── macOS (.dmg — unsigned, see §6.3) ───
  macos:
    name: macOS (${{ matrix.target }})
    runs-on: macos-latest
    strategy:
      matrix:
        include:
          - target: aarch64-apple-darwin
            arch: arm64
          - target: x86_64-apple-darwin
            arch: x86_64
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.target }}
      - uses: Swatinem/rust-cache@v2

      - name: Build
        run: cargo build --release -p unblock-app --target ${{ matrix.target }}

      # ── Signing & notarisation (disabled — requires Apple Developer Program) ──
      # Uncomment when APPLE_CERTIFICATE and related secrets are configured.
      # See §6.4 for the full signing flow.
      #
      # - name: Sign binary
      #   env:
      #     APPLE_CERTIFICATE: ${{ secrets.APPLE_CERTIFICATE }}
      #     APPLE_CERTIFICATE_PASSWORD: ${{ secrets.APPLE_CERTIFICATE_PASSWORD }}
      #     APPLE_IDENTITY: ${{ secrets.APPLE_SIGNING_IDENTITY }}
      #   run: |
      #     echo "$APPLE_CERTIFICATE" | base64 --decode > certificate.p12
      #     security create-keychain -p "" build.keychain
      #     security import certificate.p12 -k build.keychain -P "$APPLE_CERTIFICATE_PASSWORD" -T /usr/bin/codesign
      #     security set-key-partition-list -S apple-tool:,apple: -s -k "" build.keychain
      #     security default-keychain -s build.keychain
      #     codesign --force --options runtime --sign "$APPLE_IDENTITY" \
      #       target/${{ matrix.target }}/release/unblock

      - name: Create .dmg
        run: |
          brew install create-dmg
          create-dmg \
            --volname "Unblock" \
            --app-drop-link 400 100 \
            --no-internet-enable \
            "Unblock-${{ matrix.arch }}.dmg" \
            target/${{ matrix.target }}/release/unblock

      # - name: Notarise
      #   env:
      #     APPLE_ID: ${{ secrets.APPLE_ID }}
      #     APPLE_TEAM_ID: ${{ secrets.APPLE_TEAM_ID }}
      #     APPLE_APP_PASSWORD: ${{ secrets.APPLE_APP_PASSWORD }}
      #   run: |
      #     xcrun notarytool submit "Unblock-${{ matrix.arch }}.dmg" \
      #       --apple-id "$APPLE_ID" \
      #       --team-id "$APPLE_TEAM_ID" \
      #       --password "$APPLE_APP_PASSWORD" \
      #       --wait
      #     xcrun stapler staple "Unblock-${{ matrix.arch }}.dmg"

      - uses: actions/upload-artifact@v4
        with:
          name: macos-${{ matrix.arch }}
          path: "Unblock-${{ matrix.arch }}.dmg"

  # ─── Linux (AppImage) ───
  linux:
    name: Linux (${{ matrix.target }})
    runs-on: ${{ matrix.runner }}
    strategy:
      matrix:
        include:
          - target: x86_64-unknown-linux-gnu
            arch: x86_64
            runner: ubuntu-latest
          - target: aarch64-unknown-linux-gnu
            arch: aarch64
            runner: ubuntu-24.04-arm
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.target }}
      - uses: Swatinem/rust-cache@v2

      - name: Install system deps
        run: |
          sudo apt-get update
          sudo apt-get install -y \
            libvulkan-dev libfontconfig-dev \
            libxcb-shape0-dev libxcb-xfixes0-dev \
            fuse libfuse2

      - name: Build
        run: cargo build --release -p unblock-app --target ${{ matrix.target }}

      - name: Package AppImage
        run: |
          mkdir -p AppDir/usr/bin AppDir/usr/share/applications AppDir/usr/share/icons/hicolor/256x256/apps
          cp target/${{ matrix.target }}/release/unblock AppDir/usr/bin/
          cp assets/unblock.desktop AppDir/usr/share/applications/
          cp assets/unblock.png AppDir/usr/share/icons/hicolor/256x256/apps/
          ln -s usr/bin/unblock AppDir/AppRun
          ln -s usr/share/icons/hicolor/256x256/apps/unblock.png AppDir/unblock.png
          ln -s usr/share/applications/unblock.desktop AppDir/unblock.desktop
          # Download appimagetool
          wget -q "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-${{ matrix.arch }}.AppImage" -O appimagetool
          chmod +x appimagetool
          ARCH=${{ matrix.arch }} ./appimagetool AppDir "Unblock-${{ matrix.arch }}.AppImage"

      - uses: actions/upload-artifact@v4
        with:
          name: linux-${{ matrix.arch }}
          path: "Unblock-${{ matrix.arch }}.AppImage"

  # ─── GitHub Release ───
  release:
    name: Create Release
    needs: [macos, linux]
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - uses: actions/download-artifact@v4
        with:
          path: artifacts

      - name: Create GitHub Release
        uses: softprops/action-gh-release@v2
        with:
          tag_name: ${{ github.ref_name }}
          name: "Unblock Desktop ${{ github.ref_name }}"
          generate_release_notes: true
          files: |
            artifacts/macos-arm64/Unblock-arm64.dmg
            artifacts/macos-x86_64/Unblock-x86_64.dmg
            artifacts/linux-x86_64/Unblock-x86_64.AppImage
            artifacts/linux-aarch64/Unblock-aarch64.AppImage
```

### 6.3 macOS Unsigned Distribution

The desktop app ships **unsigned** initially. No Apple Developer Program subscription is required. Users will see a Gatekeeper warning on first launch.

**README install instructions for macOS:**

```markdown
### macOS

Download the `.dmg` for your architecture from
[Releases](https://github.com/websublime/unblock/releases).

Since the app is not signed with an Apple Developer certificate,
macOS will block it on first open. To allow it:

1. Open the `.dmg` and drag Unblock to Applications
2. Open Unblock — macOS will show "cannot be opened because
   the developer cannot be verified"
3. Go to **System Settings → Privacy & Security**
4. Scroll down — you'll see "Unblock was blocked". Click **Open Anyway**
5. Confirm in the dialog

Or from the terminal:
​```bash
xattr -cr /Applications/Unblock.app
​```

This only needs to be done once.
```

**Future:** If adoption justifies the investment, Apple Developer Program signing and notarisation can be enabled by adding the `APPLE_*` secrets to the repo and uncommenting the signing steps in `desktop-release.yml`. See §6.4 for the full signing flow.

### 6.4 Signing Flow (future — commented out in workflow)

When Apple Developer Program is available, uncommenting the signing steps in `desktop-release.yml` enables:

1. Import `.p12` certificate to ephemeral keychain
2. `codesign --force --options runtime --sign` the binary
3. `create-dmg` packages the signed binary
4. `xcrun notarytool submit` sends `.dmg` for notarisation
5. `xcrun stapler staple` embeds the notarisation ticket

Required secrets (see §10): `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_TEAM_ID`, `APPLE_APP_PASSWORD`.

### 6.5 Release Artifacts (per tag)

| Artifact | Platform | Notes |
|---|---|---|
| `Unblock-arm64.dmg` | macOS Apple Silicon | Unsigned (see §6.3) |
| `Unblock-x86_64.dmg` | macOS Intel | Unsigned (see §6.3) |
| `Unblock-x86_64.AppImage` | Linux x86_64 | Self-contained, Vulkan required at runtime |
| `Unblock-aarch64.AppImage` | Linux ARM64 | Self-contained, Vulkan required at runtime |

### 6.6 Install Methods

```bash
# macOS — download .dmg, drag to Applications, bypass Gatekeeper (see §6.3)
# Or via Homebrew Cask:
brew install --cask websublime/tap/unblock-desktop

# Linux — download AppImage, chmod +x, run
chmod +x Unblock-x86_64.AppImage
./Unblock-x86_64.AppImage
```

---

## 7. Distribution Channels

| Channel | MCP (`unblock-mcp`) | Desktop (`unblock-app`) |
|---|---|---|
| GitHub Releases | ✅ Tarballs + installers (cargo-dist) | ✅ `.dmg` + AppImage (custom) |
| Homebrew formula | ✅ `brew install websublime/tap/unblock-mcp` | — |
| Homebrew cask | — | ✅ `brew install --cask websublime/tap/unblock-desktop` |
| npm | ✅ `npx @unblock/cli` | ❌ |
| Shell installer | ✅ `curl \| sh` (cargo-dist) | ❌ |
| PowerShell installer | ✅ `irm \| iex` (cargo-dist) | ❌ |
| cargo install | ✅ `cargo install unblock-mcp` | ❌ (system deps) |

---

## 8. Homebrew Tap

Repository: `websublime/homebrew-tap`

### 8.1 MCP Formula (auto-updated by cargo-dist)

```ruby
# Formula/unblock-mcp.rb
# Auto-generated by cargo-dist — do not edit manually
class UnblockMcp < Formula
  desc "Dependency-aware task tracking for AI agents, powered by GitHub"
  homepage "https://github.com/websublime/unblock"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/websublime/unblock/releases/download/unblock-mcp-v#{version}/unblock-mcp-aarch64-apple-darwin.tar.xz"
    end
    on_intel do
      url "https://github.com/websublime/unblock/releases/download/unblock-mcp-v#{version}/unblock-mcp-x86_64-apple-darwin.tar.xz"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/websublime/unblock/releases/download/unblock-mcp-v#{version}/unblock-mcp-aarch64-unknown-linux-musl.tar.xz"
    end
    on_intel do
      url "https://github.com/websublime/unblock/releases/download/unblock-mcp-v#{version}/unblock-mcp-x86_64-unknown-linux-musl.tar.xz"
    end
  end

  def install
    bin.install "unblock-mcp"
  end

  test do
    assert_match "unblock-mcp", shell_output("#{bin}/unblock-mcp --version")
  end
end
```

### 8.2 Desktop Cask (manually maintained)

```ruby
# Casks/unblock-desktop.rb
cask "unblock-desktop" do
  version "2.0.0"
  arch arm: "arm64", intel: "x86_64"

  url "https://github.com/websublime/unblock/releases/download/unblock-app-v#{version}/Unblock-#{arch}.dmg"
  name "Unblock Desktop"
  desc "Dependency graph visualiser for GitHub-powered task tracking"
  homepage "https://github.com/websublime/unblock"

  app "Unblock.app"

  zap trash: [
    "~/.config/unblock",
  ]
end
```

The cask is macOS-only. Linux users download AppImage directly.

---

## 9. npm Wrapper

Package: `@unblock/cli` on npm.

Thin wrapper that downloads the platform-appropriate binary on `postinstall` and proxies execution.

```json
{
  "name": "@unblock/cli",
  "version": "1.0.0",
  "description": "Dependency-aware task tracking for AI agents",
  "bin": { "unblock-mcp": "./bin/unblock-mcp" },
  "scripts": {
    "postinstall": "node scripts/install.js"
  }
}
```

`scripts/install.js` detects `process.platform` + `process.arch`, downloads the matching binary from the GitHub Release, and places it in `./bin/`. Standard pattern used by `esbuild`, `turbo`, etc.

Usage:

```bash
npx @unblock/cli ready --json
# or
npm install -g @unblock/cli
unblock-mcp ready --json
```

---

## 10. Secrets and Signing

### 10.1 Required Secrets

| Secret | Used by | Purpose | Status |
|---|---|---|---|
| `HOMEBREW_TAP_TOKEN` | MCP release (cargo-dist) | PAT with write access to `websublime/homebrew-tap` | Required |
| `NPM_TOKEN` | npm publish | npm automation token for `@unblock/cli` | Required |
| `CARGO_REGISTRY_TOKEN` | Optional | For `cargo publish` to crates.io | Optional |

### 10.2 Future Secrets (Apple Developer Program)

Not required until code signing is enabled. See §6.4.

| Secret | Purpose |
|---|---|
| `APPLE_CERTIFICATE` | Base64-encoded `.p12` Developer ID certificate |
| `APPLE_CERTIFICATE_PASSWORD` | Password for the `.p12` |
| `APPLE_SIGNING_IDENTITY` | e.g. `"Developer ID Application: Websublime (XXXXXXXXXX)"` |
| `APPLE_ID` | Apple ID email for notarisation |
| `APPLE_TEAM_ID` | Apple Developer Team ID |
| `APPLE_APP_PASSWORD` | App-specific password for notarisation |

### 10.3 Secret Rotation

npm and Homebrew tokens should use fine-grained permissions (repo-scoped, package-scoped). When Apple signing is enabled, certificates expire annually — set a calendar reminder to renew and update the GitHub secret before expiry.

---

## 11. Workflow Files Summary

```
.github/workflows/
├── ci.yml                    # Push/PR: fmt, clippy, test, coverage
├── release.yml               # MCP release (auto-generated by cargo-dist)
└── desktop-release.yml       # Desktop release (custom, tag-triggered)
```

| Workflow | Trigger | Products | Tool |
|---|---|---|---|
| `ci.yml` | Push to main, all PRs | All crates | cargo test, clippy, tarpaulin |
| `release.yml` | Tag `unblock-mcp-v*` | MCP binary (5 targets) + installers | cargo-dist |
| `desktop-release.yml` | Tag `unblock-app-v*` | Desktop app (4 targets) + packages | Custom (codesign, create-dmg, appimagetool) |

### Release Checklist

```bash
# 1. MCP release
cargo release -p unblock-mcp --execute 1.1.0
# Wait for CI green → release.yml runs → artifacts published

# 2. Desktop release
cargo release -p unblock-app --execute 2.1.0
# Wait for CI green → desktop-release.yml runs → artifacts published

# 3. Post-release
# - Verify Homebrew: brew update && brew install websublime/tap/unblock-mcp
# - Verify npm: npx @unblock/cli --version
# - Verify desktop: download .dmg, open, bypass Gatekeeper, app launches
# - Update README badges if major version
```
