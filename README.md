# OPC DA Client CLI

[![Crates.io](https://img.shields.io/crates/v/opc-cli.svg)](https://crates.io/crates/opc-cli)
[![Docs.rs](https://docs.rs/opc-cli/badge.svg)](https://docs.rs/opc-cli)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

A modern, asynchronous TUI (Terminal User Interface) client for browsing, reading, and writing OPC DA (Data Access) tags on Windows.

## 🏗️ Architecture

The project is structured as a Cargo workspace with two crates:

- **`opc-cli`**: The interactive TUI application built with `ratatui` + `crossterm`.
- **`bytehound-opc-da-client`**: A ByteHound-maintained distribution of the native Windows COM library (using `windows-rs`) that abstracts OPC DA communication through an async trait (`OpcProvider`). Its Rust library name remains `opc_da_client`; the package is aliased as `opc-da-client` by consumers. Generic over `ServerConnector` for easy mocking.

See **[bytehound-opc-da-client architecture.md](./opc-da-client/architecture.md)** for the full library design, state machine, and data flow diagrams.

## ✨ Features

- **Server Discovery**: Enumerate OPC DA servers on local or remote hosts.
- **Hierarchical Browsing**: Recursive tag discovery for the TUI plus bounded, one-level native browse pages with OPC DA 3.0 support and a one-time, narrowly classified DA 2.x compatibility fallback before the first successful root page.
- **Inventory Telemetry**: Library consumers can pace bounded inventory operations, adjust their batch size at runtime, and receive typed observations for each completed slice.
- **Real-time Monitoring**: Live tag value updates with 1-second auto-refresh.
- **Tag Write Support**: Write typed values (int, float, bool, string) to individual tags.
- **Search & Filter**: Substring search with `Tab`/`Shift+Tab` cycling through matches.
- **Rich Error Hints**: Human-readable explanations for cryptic Windows COM/DCOM HRESULT codes.
- **Transparent COM Management**: COM initialization, apartment thread affinity, single-owner native result conversion, owned VQT write values, and nested allocation cleanup are handled automatically by a dedicated worker; hosts performing additional COM work can initialize their own thread with `ComGuard`.
- **Mockable Backend**: Unit-test the TUI on any OS without a live OPC server.
- **Opt-in Native Read Canary**: `bytehound-opc-da-client`'s `dev-diagnostics` feature provides a read-only direct-COM canary with JSON Lines output and a normal-worker comparison.

## 🚀 Getting Started

### Prerequisites

- **Windows OS**: This application uses Windows COM/DCOM.
- **OPC Core Components**: Must be installed on the system to resolve OPC ProgIDs.
- **Rust 1.88+**: Edition 2024.

### Build & Run

```powershell
# Run the TUI
cargo run --bin opc-cli

# Run the TUI with debug logging enabled (default is info)
cargo run --bin opc-cli -- -v

# Run the TUI with verbose trace logging enabled (captures detailed argument dumps)
cargo run --bin opc-cli -- -vv

# Run the full verification gate (format → lint → test)
pwsh -File scripts/verify.ps1
```

### Native OPC DA Diagnostics

On a Windows host with the target OPC server registered, run the feature-gated,
non-interactive canary with a ProgID and one or more exact ItemIDs:

```powershell
cargo run -p bytehound-opc-da-client --example native_read_canary --features dev-diagnostics -- Yokogawa.CSHIS_OPC.1 FCS0201!204FI00510.PV
```

The JSON Lines output reports native server/group/item metadata, explicit device and cache
reads over two server-revised update intervals, HRESULT text from Windows and the vendor,
standard item properties when supported, group cleanup, and a comparison with the normal
`OpcDaClient` worker read. The canary does not write tags and does not fall back from device
reads to cache reads.

The same example supports a bounded, read-only inventory mode:

```powershell
cargo run -p bytehound-opc-da-client --example native_read_canary --features dev-diagnostics -- `
  inventory Yokogawa.CSHIS_OPC.1 `
  --start-path FCS0219 --start-path 203FI02005 `
  --batch-size 25 --max-entries 1000 --deadline-secs 60
```

Inventory output is JSON Lines with explicit terminal and worker-lifecycle records; channel
EOF, stream errors, deadlines, and blocked-worker detachment are reported as failures rather
than clean completion.
Repeated `--start-path <COMPONENT>` values are a diagnostic-only DA2 starting path. This
targeted mode preserves exact ItemIDs and breadcrumbs while testing a nested branch directly;
without it, inventory starts at the namespace root and retains the stable provider API and
normal production behavior.


## ⌨️ Controls

| Key | Action | Screen |
| :--- | :--- | :--- |
| `Enter` | Navigate forward / Confirm input | All |
| `Esc` | Navigate back | All |
| `Space` | Toggle tag selection | Tag List |
| `s` | Enter search/filter mode | Tag List |
| `Tab` / `Shift+Tab` | Cycle through search matches | Tag List (search) |
| `w` | Enter write mode for selected tag | Tag Values |
| `↑` / `↓` | Navigate lists | All lists |
| `PgUp` / `PgDn` | Page through lists (20 items) | All lists |
| `q` / `Q` | Quit application | Home |

## 📦 Packaging & Deployment

The repository supports two release packaging models:

### 1. Modern Release (Windows 10+ / Server 2016+)

```powershell
make package
# OR
pwsh -File scripts/package.ps1 package
```
Output: `dist/opc-cli-x64/` and `dist/opc-cli-x64.zip`

### 2. Legacy Release (Windows 7 SP1 / Server 2008 R2 SP1)

For deployment to offline, air-gapped industrial environments running Windows 7 / Server 2008 R2 (NT 6.1):

```powershell
make package-win7
# OR
pwsh -File scripts/package.ps1 package-win7
```
Output: `dist/opc-cli-win7-x64/` and `dist/opc-cli-win7-x64.zip`

**Legacy Bundle Contents:**
- `opc-cli.exe`: PE-patched executable linked with static CRT (`+crt-static`). Replaces missing `GetSystemTimePreciseAsFileTime` imports with native `GetSystemTimeAsFileTime`.
- `api-ms-win-core-synch-l1-2-0.dll`: `#![no_std]` polyfill for `WaitOnAddress` and `Sleep` re-export.
- `api-ms-win-core-winrt-error-l1-1-0.dll`: `#![no_std]` no-op stubs for WinRT error APIs.
- `bcryptprimitives.dll`: `#![no_std]` polyfill routing `ProcessPrng` to `RtlGenRandom` (`advapi32.dll`).
- `redist/`: Included OPC Core Components redistributable MSI (if placed in `vendor/redist/`).

Simply copy the extracted `dist/opc-cli-win7-x64/` folder to a USB drive and run on the target machine without installing Visual C++ redistributables or Windows updates.

## 🙏 Acknowledgments

- [**rust_opc**](https://github.com/Ronbb/rust_opc) by Wang Ruobiao — original OPC DA Rust bindings and COM interface generation pipeline.
- [**OPC Foundation**](https://opcfoundation.org/) — OPC Data Access specification and IDL interface definitions.
- [**windows-rs**](https://github.com/microsoft/windows-rs) by Microsoft — Windows API bindings for Rust.
- [**ratatui**](https://github.com/ratatui/ratatui) — terminal user interface framework.

## 📄 License

This project is licensed under the MIT License — see the [LICENSE](LICENSE) file for details.
