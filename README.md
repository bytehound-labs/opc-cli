# OPC DA Client CLI

[![Crates.io](https://img.shields.io/crates/v/opc-cli.svg)](https://crates.io/crates/opc-cli)
[![Docs.rs](https://docs.rs/opc-cli/badge.svg)](https://docs.rs/opc-cli)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

A modern, asynchronous TUI (Terminal User Interface) client for browsing, reading, and writing OPC DA (Data Access) tags on Windows.

## 🏗️ Architecture

The project is structured as a Cargo workspace with two crates:

- **`opc-cli`**: The interactive TUI application built with `ratatui` + `crossterm`.
- **`bytehound-opc-da-client`**: A ByteHound-maintained distribution of the native Windows COM library (using `windows-rs`) that abstracts OPC DA communication through an async trait (`OpcProvider`). Its Rust library name remains `opc_da_client`; the package is aliased as `opc-da-client` by consumers. Generic over `ServerConnector` for easy mocking.

See the library's [architecture](./opc-da-client/docs/architecture.md) and
[behavioral contract](./opc-da-client/docs/spec.md) for domain ownership,
COM lifetimes, browsing, inventory, and platform guarantees. Both documents
are included in the library package.

## ✨ Features

- **Server Discovery**: Enumerate OPC DA servers registered on the native Windows client machine.
- **Hierarchical Browsing**: Recursive tag discovery for the TUI plus bounded, one-level native browse pages with OPC DA 3.0 support and a one-time, narrowly classified DA 2.x compatibility fallback before the first successful root page.
- **Direct DA2 Inventory Navigation**: Large hierarchical inventories use canonical `OPC_BROWSE_TO` navigation first, fall back only for known compatibility errors, defer branch expansion, and preserve same-named branch/item nodes without eager probe traffic.
- **Root-Scoped Inventory**: Diagnostic callers can stream a bounded inventory from one exact canonical ItemID through `OpcProvider::start_inventory_at_root`, using a fresh independent OPC connection without parsing vendor-specific `.`, `!`, or `/` separators.
- **Inventory Telemetry**: Library consumers can pace bounded inventory operations, adjust their batch size at runtime, and receive typed per-operation observations for each completed slice plus separate startup capability-detection observations. Summaries include counts, elapsed-time totals, maxima, fixed latency buckets, and approximate p50/p95/p99 values. Best-effort numeric collectors recover poisoned locks without failing traversal.
- **Inventory Pacing Accounting**: DA3 pages are charged by their requested page size. DA2 enumeration is charged only when the native iterator refills, using its cache capacity; values already held in that cache are free and cancellation is still checked between them.
- **Quiet Normal Operation**: Successful list, read, write, and browse operations are debug-level events; failures remain visible at warning or error level without producing one informational record per inventory operation.
- **Bounded Browse Safety**: Native and compatibility browse iterators stop after 64 consecutive identical values and report the iterator, browse path, repeated value, and progress counters instead of stalling indefinitely.
- **DA2 Branch Recovery**: A non-progressing DA2 branch iterator is discarded while the independent item iterator continues; item-side non-progress and unrelated errors remain terminal, and the completion warning records the skipped branch iterator.
- **COM Iterator Ownership**: Browse buffers are cleared as entries are consumed, malformed native counts are rejected before indexing, and remaining COM-allocated strings are released after failed or early-ended traversal.
- **Restartable Inventory Lifecycle**: Startup failures, cancellation, and worker unwinding release the active inventory state so later inventory attempts are not blocked by stale ownership.
- **Real-time Monitoring**: Live tag value updates with 1-second auto-refresh.
- **Tag Write Support**: Write typed values (int, float, bool, string) to individual tags.
- **Search & Filter**: Substring search with `Tab`/`Shift+Tab` cycling through matches.
- **Rich Error Hints**: Human-readable explanations for cryptic Windows COM/DCOM HRESULT codes, with the native Windows error retained as the error source.
- **Transparent COM Management**: COM initialization and apartment thread affinity are handled automatically by a dedicated worker; hosts performing additional COM work can initialize their own thread with `ComGuard`.
- **Mockable Backend**: Test portable provider/models on Linux and native worker/TUI behavior on Windows without a live OPC server.

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

The workspace and TUI target Windows. To verify the publishable library package on Linux without
compiling the Windows COM backend, scope Cargo commands to the library crate:

```bash
cargo test -p bytehound-opc-da-client --all-features
cargo clippy -p bytehound-opc-da-client --all-targets --all-features -- -D warnings
cargo publish -p bytehound-opc-da-client --dry-run
```

These Linux commands compile and test the actual provider trait, value and error models,
opaque browse tokens, inventory controls, streams, and telemetry summaries. The optional
`test-support` mock provider also works off Windows. Windows verification remains required
for the native OPC DA backend and TUI.
Platform selection preserves Windows error sources, HRESULT bit patterns and hints,
and structured log fields.
Windows CI uses stable Rust with warnings denied for application and test code, and checks both
the native workspace target and 32-bit Windows consumers.
Mock-backend tests cover worker-thread connection ownership, browse-session cancellation, and
inventory stream cleanup without contacting a live OPC server. Tracing assertions release
event collector locks before validating captured metadata.

The native implementation separates lifecycle/orchestration from connection,
read/write, DA2/DA3 navigation, continuation, and pacing/telemetry modules.
Public API paths, MTA ownership, foreground connection isolation, and established
tracing targets stay consistent across those module boundaries.

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
