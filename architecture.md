# 🏗️ Architecture: opc-cli

## 1. Project Overview
`opc-cli` is a high-performance, asynchronous Terminal User Interface (TUI) application designed for interacting with OPC DA (Data Access) 2.05a/3.0 servers on Windows. It combines a responsive terminal workspace (`ratatui` + `crossterm`) with a native Windows COM client library (`opc-da-client`) to discover OPC DA servers, recursively browse tag namespaces, monitor real-time value changes, and perform typed tag writes.

## 2. Project Objectives & Key Features

### Primary Objectives
- **Zero-Crash Reliability**: Maintain non-blocking async operation with safe COM thread-apartment isolation so TUI rendering never freezes during slow network or server operations.
- **Legacy Operating System Compatibility**: Full deployment support for legacy NT 6.1 industrial control systems (Windows 7 SP1 / Windows Server 2008 R2 SP1) alongside modern Windows 10/11/Server 2022 targets.
- **Cross-Platform Testability**: Abstract all OPC interactions behind mockable Rust traits so UI state transitions and application logic can be 100% verified on any operating system without a live COM server.

### Key Features
- **Server Discovery**: Enumerate local or remote OPC DA servers registered in the Windows COM registry.
- **Namespace Browsing**: Fast-path flat address space browsing (`OPC_FLAT`) with recursive depth-first fallback and partial-result harvesting on timeout.
- **Real-Time Tag Monitoring**: Continuous 1-second background auto-refresh with value, quality bitmask, and timestamp formatting.
- **Typed Tag Writing**: Interactive write mode supporting `String`, `Int` (`VT_I4`), `Float` (`VT_R8`), and `Bool` (`VT_BOOL`) values.
- **Rich COM Diagnostics**: Automated HRESULT mapping (`friendly_com_hint`) providing human-readable explanations for cryptic DCOM and OPC error codes.

### Target Users / Audience
- **Control Engineers & Automation Technicians**: Inspecting live OPC DA server tags during commissioning and troubleshooting on shop-floor control systems.
- **SCADA Developers**: Validating server connectivity, item ID syntax, and read/write permissions.
- **Industrial Systems Administrators**: Deploying lightweight diagnostics into air-gapped legacy Windows 7 environments without requiring full SCADA installations or VC++ redistributable packages.

### Non-Goals
- **OPC UA Support**: `opc-cli` explicitly focuses on classic OPC DA (COM/DCOM). OPC UA (TCP/binary) is out of scope.
- **Historical Data Access (OPC HDA)** or **Alarms & Events (OPC A&E)**: Out of scope.

## 3. Language & Runtime
* **Language**: Rust (Edition 2024, MSRV 1.93.1).
* **OS Target**: Windows (Strict) due to OPC DA reliance on Windows COM/DCOM (`windows` crate 0.61.3).
* **Async Runtime**: `tokio` (Multi-thread runtime).
* **TUI Engine**: `ratatui` (v0.29) + `crossterm` (v0.28).

## 4. Project Layout

```
opc-cli/
├── Cargo.toml                  # Workspace root configuration & shared dependencies
├── Makefile                    # Unified CLI frontend for developers (delegates to scripts/)
├── README.md                   # Workspace repository documentation
├── architecture.md             # Technical Source of Truth & architecture specifications
├── context.md                  # Historical decisions & TARS interaction log
├── spec.md                     # Behavioral contracts (if workspace-level)
├── opc-cli/                    # Interactive TUI Application Crate
│   ├── Cargo.toml              # App dependencies (ratatui, crossterm, clap)
│   └── src/
│       ├── main.rs             # Application entrypoint & CLI argument parsing
│       ├── app.rs              # App state machine, event loop & background task polling
│       └── ui.rs               # Ratatui view render functions
├── opc-da-client/              # Native OPC DA Client Library Crate
│   ├── Cargo.toml              # Library dependencies (windows, anyhow, thiserror)
│   ├── spec.md                 # Library behavioral contracts
│   └── src/
│       ├── lib.rs              # Library root & public re-exports
│       ├── provider.rs         # OpcProvider trait, TagValue, OpcValue, WriteResult
│       ├── com_worker.rs       # Dedicated COM MTA worker thread & connection pool
│       ├── com_guard.rs        # RAII CoInitializeEx / CoUninitialize guard
│       └── helpers.rs          # HRESULT hints, VARIANT/FILETIME formatters
├── compat/                     # Windows 7 / NT 6.1 Polyfill DLL Crates (#![no_std])
│   ├── bcrypt-polyfill/       # ProcessPrng -> RtlGenRandom polyfill
│   ├── synch-polyfill/        # WaitOnAddress 1ms Sleep polling polyfill
│   └── winrt-error-polyfill/  # RoOriginateError S_OK stub polyfill
└── scripts/                    # Automation & Quality Gate Pipelines
    ├── package.ps1             # Universal task dispatcher (single source of truth)
    ├── package-win7.ps1        # Standalone NT 6.1 legacy release pipeline & PE patcher
    ├── verify.ps1              # 5-gate quality pipeline runner
    ├── check-logs.ps1           # Log inspector & statistical analyzer
    ├── commit.ps1             # Quality-gated commit & push pipeline
    └── Merge-ToMain.ps1        # Clean release merger dev -> main
```

## 5. Module Boundaries

### `opc-cli` (TUI Application)
- **Owns**: Terminal UI rendering, keyboard input handling, navigation state machine, background async task spawning (`tokio::spawn`), status bar notifications.
- **Does NOT Own**: Raw COM initialization, registry enumeration, OPC group creation, HRESULT interpretation logic.
- **Trait Interfaces**: Consumes `dyn OpcProvider` asynchronously.
- **Mock Availability**: Fully mockable via `MockOpcProvider` (compiled when `feature = "test-support"` is active in `opc-da-client`).

### `opc-da-client` (Core Client Library)
- **Owns**: Public API (`OpcProvider`), data structs (`TagValue`, `OpcValue`, `WriteResult`), error definitions (`OpcError`), COM hint engine (`friendly_com_hint`).
- **Does NOT Own**: Terminal rendering, direct COM worker loop implementation.
- **Trait Interfaces**: Exports `OpcProvider`.
- **Mock Availability**: Provides `MockOpcProvider` via `mockall`.

### `ComWorker` (MTA Worker Thread Pool)
- **Owns**: Dedicated OS background thread, `CoInitializeEx(MTA)` lifecycle (`ComGuard`), connection pool caching (`HashMap<ProgID, Server>`), transparent stale connection eviction on RPC errors (`0x800706BA`), tag browse walking.
- **Does NOT Own**: TUI state, UI rendering, high-level task timeouts.
- **Trait Interfaces**: Uses internal `ServerConnector` trait.
- **Mock Availability**: Fully unit-tested via `ConfigurableMockConnector`.

### `compat/*` (NT 6.1 Polyfill Crates)
- **Owns**: C-ABI DLL exports for missing Windows 8+ APIs (`WaitOnAddress`, `ProcessPrng`, `RoOriginateError`).
- **Does NOT Own**: Standard Rust library (`#![no_std]`), workspace Cargo builds (excluded from workspace).
- **Trait Interfaces**: C-ABI Exported DLL functions.
- **Mock Availability**: Tested via `verify.ps1` standalone release builds.

## 6. Dependency Direction Rules

| Module | May Import | Must NOT Import |
|:---|:---|:---|
| `opc-cli` (TUI App) | `opc-da-client` (`OpcProvider` trait, `OpcValue`, `TagValue`, `WriteResult`, `OpcError`), `ratatui`, `crossterm`, `tokio`, `tracing` | Direct Windows COM APIs (`windows::Win32::System::Com`), `backend::opc_da` concrete types |
| `opc-da-client::provider` | `thiserror`, `chrono`, `serde` | `windows`, `ratatui`, `crossterm`, `tokio` |
| `opc-da-client::com_worker` | `windows`, `windows-core`, `provider` types, `helpers`, `tokio::sync` | `opc-cli`, `ratatui`, `crossterm` |
| `compat/*` (Polyfills) | `core`, `windows-sys` / raw Win32 FFI | `std`, `tokio`, `opc-cli`, `opc-da-client` |

## 7. Toolchain

The project uses a unified dual-interface build system:

1. **Makefile**: The primary CLI entry point for developers. All complex multi-step workflows delegate directly to PowerShell scripts.
   - `make debug`: Fast development build (`cargo build`).
   - `make release` / `make build`: Optimized production build (`cargo build --release`).
   - `make test`: Quick unit test run (`cargo test`).
   - `make verify`: Executes 5-gate quality pipeline (`pwsh scripts/verify.ps1`).
   - `make package`: Builds modern (Win10+) release bundle into `dist/opc-cli-x64.zip`.
   - `make package-win7`: Builds legacy (Win7/Server 2008 R2) release bundle into `dist/opc-cli-win7-x64.zip`.
   - `make logs`: Runs log inspector (`pwsh scripts/check-logs.ps1`).
   - `make commit MSG="..."`: Runs quality gate, commits, and pushes to remote (`pwsh scripts/commit.ps1`).
   - `make release-merge`: Clean release merge from `dev` to `main` (`pwsh scripts/Merge-ToMain.ps1`).
   - `make clean`: Cleans build artifacts and `dist/` directory.

2. **scripts/package.ps1**: Single PowerShell task dispatcher for all workspace operations.
   - Usage: `pwsh -File ./scripts/package.ps1 -Task <task>`
   - Supported tasks: `debug`, `release`, `build`, `test`, `verify`, `package`, `package-win7`, `logs`, `commit`, `release-merge`.

3. **scripts/package-win7.ps1**: Dedicated legacy packaging pipeline that compiles polyfills, PE-patches the binary, and bundles redistributables.
4. **scripts/verify.ps1**: Universal 5-gate quality pipeline (formatter, linter, doc-tests, workspace tests, polyfill compilation).
5. **scripts/check-logs.ps1**: Log inspector and deep analysis utility.
6. **scripts/commit.ps1**: Quality-gated commit & push pipeline.
7. **scripts/Merge-ToMain.ps1**: Automated clean release merge tool.

## 8. Error Handling Strategy

- **Library Domain Errors**: `OpcError` (defined in `opc-da-client`) handles domain failures via `thiserror`.
- **Friendly Hint Engine**: `friendly_com_hint()` maps technical HRESULT codes (e.g. `0x800706BA` RPC Unavailable, `0x80070005` DCOM Access Denied) to actionable plain-English text.
- **Breadcrumb Chains**: TUI uses `anyhow` displaying `{:#}` full error chains in status popups.
- **No Swallowed Errors**: All fallible COM and background task operations propagate `Result<T, OpcError>`.

## 9. Observability & Logging

- **Framework**: `tracing` + `tracing-subscriber` + `tracing-appender-localtime`.
- **Target**: Rolling log file `logs/opc-cli.log` (stdout is reserved for Ratatui TUI rendering).
- **Instrumentation**: Key operations (`list_servers`, `browse_tags`, `read_tag_values`, `write_tag_value`) log entry, exit, and `elapsed_ms` execution timing.
- **Log Inspector**: `scripts/check-logs.ps1` provides CLI log scanning, severity filtering (`WARN`/`ERROR`), lifecycle sequence checks, and timing outlier statistics.

## 10. Testing Strategy

- **Unit Testing**: Mock-based testing using `MockOpcProvider` (`mockall`). TUI navigation flow, state transitions, search cycling, and ring-buffer logic are verified without Windows COM dependencies (34 unit tests in `opc-cli`).
- **COM Worker Testing**: `ComWorker` unit tests (`opc-da-client/src/com_worker.rs`) use `ConfigurableMockConnector` to test write paths, server connection pooling (`connect_count == 1`), stale connection eviction (`connect_count == 2`), thread panic safety, and worker drop behaviors (37 unit tests in `opc-da-client`).
- **Doc Testing**: Public API items include runnable doc tests (`cargo test --doc`).
- **Polyfill Build Gates**: Independent compilation of `compat/*` polyfill crates inside `scripts/verify.ps1`.

## 11. Documentation Conventions

- **Rustdoc Comments**: All public types and methods require `///` doc comments detailing purpose, arguments, returns, and errors. Crate roots require `//!` module overviews.
- **Behavioral Contracts**: `spec.md` files (e.g. `opc-da-client/spec.md`) maintain the behavioral contracts for public traits and structs, verified against source code via `> Last verified against: <hash>`.
- **Architecture Sync**: `architecture.md` serves as the Technical Source of Truth for system layout and design patterns.

## 12. Dependencies & External Systems

- **Windows COM/DCOM**: Core OS dependency for OPC DA. Requires registered OPC Core Components (`opcproxy.dll`, `opccomn_ps.dll`).
- **`windows` crate (0.61.3)**: Windows Win32 API bindings (`Win32_System_Com`, `Win32_System_Variant`, `Win32_System_Ole`).
- **`ratatui` (0.29.0) / `crossterm` (0.28.1)**: Terminal user interface framework.

## 13. Architecture Diagrams

### Application State Flow
```mermaid
stateDiagram-v2
    [*] --> Init
    Init --> Home : App Start
    
    state "Home (Enter Hostname)" as Home {
        [*] --> InputWait
        InputWait --> Connecting : Enter Key
        Connecting --> InputWait : Error (Update Status)
    }

    Home --> ServerList : Success (Servers Found)

    state "Server List" as ServerList {
        [*] --> NavigatingServers
        NavigatingServers --> BrowsingTags : Enter Key (Select Server)
        NavigatingServers --> Home : Esc Key
    }

    ServerList --> TagList : Success (Tags Found)

    state "Tag List" as TagList {
        [*] --> NavigatingTags
        NavigatingTags --> SearchMode : S Key
        SearchMode --> NavigatingTags : Esc Key
        NavigatingTags --> ReadingValues : Enter Key
        NavigatingTags --> ServerList : Esc Key
    }

    TagList --> TagValues : Success (Values Read)

    state "Tag Values" as TagValues {
        [*] --> ViewingValues
        ViewingValues --> WriteInput : W Key
        ViewingValues --> TagList : Esc Key
    }

    state "Write Input" as WriteInput {
        [*] --> EnteringValue
        EnteringValue --> Writing : Enter Key
        Writing --> TagValues : Success (refresh)
        Writing --> TagValues : Error (show message)
        EnteringValue --> TagValues : Esc Key
    }

    Home --> [*] : Esc Key (Quit)
```

### Data Flow
```mermaid
graph TD
    User[User Input] --> |Key/Mouse Event| EventLoop[Main Event Loop]
    EventLoop --> |Dispatch| AppUpdate[App::update()]
    
    subgraph Core Logic
        AppUpdate --> |Request Data| OpcProvider[Trait: OpcProvider]
        OpcProvider --> |Call| Lib[opc-da-client]
        Lib --> |COM/DCOM| Server[OPC Server]
        Server --> |Data| Lib
        Lib --> |Result| AppUpdate
        AppUpdate --> |Mutate| AppState[App State Model]
    end
    CLI["opc-cli"]
    subgraph "opc-da-client"
        Provider["trait OpcProvider"]
        Backend["backend::opc_da"]
        OpcDaInternal["opc_da (merged)"]
        Bindings["bindings (merged)"]
    end
    CLI --> Provider --> Backend --> OpcDaInternal
    OpcDaInternal --> Bindings --> WinCOM["Windows COM/DCOM"]
    
    subgraph Rendering
        AppState --> |Read| View[UI Render Functions]
        View --> |Draw| Terminal[Ratatui / Crossterm]
    end

    subgraph Logging
        AppUpdate --> |Log| Tracing
        OpcProvider --> |Log| Tracing
        Tracing --> |Write| LogFile[logs/opc-cli.log]
    end
```

### Error Propagation Flow
```mermaid
sequenceDiagram
    autonumber
    participant Server as OPC DA Server
    participant Worker as ComWorker (MTA Thread)
    participant Client as OpcDaClient
    participant App as App Event Loop (Tokio)
    participant UI as TUI Status Bar

    Server-->>Worker: COM Failure (e.g. HRESULT 0x800706BA)
    Worker->>Worker: Check is_connection_error() -> Evict stale server handle
    Worker-->>Client: OpcError::Com { source }
    Client->>Client: friendly_com_hint(&err) -> "RPC server unavailable..."
    Client-->>App: Err(anyhow::Error with context)
    App->>App: Format error chain {:#}
    App-->>UI: Display friendly hint & breadcrumb on Status Bar
```

## 14. Known Constraints & Technical Debt

- **NT 6.1 Import Patching**: Windows 7 lacks `GetSystemTimePreciseAsFileTime`. `scripts/package-win7.ps1` binary-patches the import table to `GetSystemTimeAsFileTime`.
- **StringIterator Bug Workaround (OPC-BUG-001)**: Handled internally by `StringIterator` zeroing cache and skipping null `PWSTR` entries.
- **Windows COM Single-Threaded Apartment Constraints**: Managed by routing all COM operations through `ComWorker` on a dedicated MTA thread.

## 15. Data Model
- Application state is managed in-memory via `App` struct model. No persistent database or SQL storage is required.

## 16. Environment Configuration
- Local Windows console execution. Configuration parameters (target hostname, max tags, timeouts) are supplied via CLI flags (`clap`) or UI prompt input.
