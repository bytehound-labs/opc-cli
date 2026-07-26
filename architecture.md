# 🏗️ Architecture: opc-cli

## High-Level Overview
`opc-cli` is a command-line interface tool designed to interact with OPC DA (Data Access) servers. It provides a Terminal User Interface (TUI) for browsing servers and tags, reading values, and monitoring system status.

## Technology Stack
*   **Language**: Rust (2024 Edition)
*   **OS Target**: Windows (Strict) due to OPC DA reliance on COM/DCOM.
*   **TUI Framework**: `ratatui` + `crossterm`.

## Core Components & Crates

### 1. User Interface (TUI)
*   **Crate**: `ratatui`, `crossterm`
*   **Purpose**: Renders the terminal interface and handles keyboard/mouse events.

### 2. Client Library
*   **Crate**: `opc-da-client`
*   **Purpose**: Provides a unified, backend-agnostic trait (`OpcProvider`) for OPC DA operations.

### 3. Core Logic & Async Runtime
*   **Crate**: `tokio`
*   **Responsibility**: Driving the main event loop and handling async tasks (though COM interactions are often thread-bound).

*   **Layer**: `opc-da-client` (Stable library crate)
*   **Responsibility**: Communicating with Local/Remote OPC Servers.
*   **COM Safety**: `ComGuard` (internal RAII guard in `opc-da-client/src/com_guard.rs`) ensures `CoUninitialize` is called exactly once per successful `CoInitializeEx` on the worker thread, even on panics.
*   **Abstraction**: `trait OpcProvider` (defined in `opc-da-client/src/provider.rs`)
    *   Decouples the UI from the specific OPC implementation.
    *   Enables **Testability** via `mockall` (allowing UI development on any OS).
    *   **Backend Swappability**: Underlying OPC stack can be swapped without changing CLI code.
    *   **Methods:**
        | Method | Purpose |
        | :--- | :--- |
        | `list_servers(host)` | Enumerate OPC DA servers from the registry |
        | `browse_tags(server, max_tags, progress, tags_sink)` | Recursively walk the tag namespace; pushes to a shared sink for partial-result harvesting on timeout |
        | `read_tag_values(server, tag_ids)` | SyncIO read of selected tags, returning value/quality/timestamp |
        | `write_tag_value(server, tag_id, value)` | Write a typed value (`OpcValue`) to a single tag |

#### Browse Strategy
The browse implementation handles both flat and hierarchical OPC DA namespaces:
1. `query_organization()` detects namespace type (flat vs hierarchical).
2. **Flat:** Enumerate all `OPC_LEAF` items at root.
3. **Hierarchical — OPC_FLAT fast path (preferred):**
   Try `BrowseOPCItemIDs(OPC_FLAT)` at root — returns ALL leaf items as fully-qualified IDs in a single pass. Falls back to recursive browse if the server returns an error or empty results.
4. **Hierarchical — Recursive fallback:**
   Depth-first walk via `browse_recursive()`:
   - **Branches first:** Enumerate `OPC_BRANCH` items, navigate down via `change_browse_position(DOWN)`, recurse, then always navigate back `UP` — even if recursion fails — to prevent position corruption.
   - **Leaves second (soft-fail):** Enumerate `OPC_LEAF` items at current position; failures are logged and skipped.
   - **Fully-qualified IDs:** `get_item_id()` converts browse names to item IDs; falls back to browse name if conversion fails.
   - **Iterator bug (OPC-BUG-001) — FIXED:** `StringIterator` now zeroes its cache before each `IEnumString::Next()` call and silently skips null `PWSTR` entries, eliminating the phantom `E_POINTER` errors at the iterator level.
5. **Safety guards:**
   - `max_tags` hard cap (default 10000) to prevent unbounded collection.
   - `MAX_DEPTH` (50) to guard against infinite recursion in malformed namespaces.
   - **300-second timeout** with graceful partial result harvesting. If tags are discovered before the timeout, the application displays the partial list with a warning instead of an error.
   - A shared `tags_sink` (`Arc<Mutex<Vec<String>>>`) allows the main thread to harvest tags mid-browse on timeout.
6. **Non-blocking:** Browse runs as a background task; progress reported via `Arc<AtomicUsize>` to the Loading screen.
6. **Decoupled Architecture**: Detailed browse logic and COM handling live in `opc-da-client`. See [architecture: opc-da-client](file:///c:/Users/WSALIGAN/code/opc-cli/opc-da-client/architecture.md) for specifics.

#### TUI Interaction Features
| Feature | Key(s) | Screens | Behavior |
| :--- | :--- | :--- | :--- |
| Navigation | `↑` / `↓` | All lists | Move selection by 1 item |
| Fast Scroll | `PageUp` / `PageDown` | ServerList, TagList, TagValues | Jump by 20 items (clamped) |
| Tag Search | `s` | TagList | Enter modal substring search |
| Search Cycle | `Tab` / `Shift+Tab` | TagList (search mode) | Jump between matches |
| Toggle Select | `Space` | TagList | Check/uncheck tag for reading |
| Read Values | `Enter` | TagList | Read selected tags from server |
| Write Value | `w` | TagValues | Enter write mode for the selected tag |
| Back | `Esc` | All | Navigate to previous screen |


### 4. Observability
*   **Crate**: `tracing`, `tracing-subscriber`, `tracing-appender`
*   **Responsibility**: Structured logging to **File** (`opc-cli.log`).
    *   **Timing Instrumentation**: Key COM operations (`create_server`, `query_organization`, `browse`) are wrapped in `Instant` timers. Success logs include `elapsed_ms` to identify server performance bottlenecks.
    *   **Context Preservation**: All errors are logged at the point of origin with raw HRESULT codes before being wrapped for the UI.
    *   *Critical*: Since TUI captures stdout/stderr, logs must go to a file for debugging crashes or connection issues.

### 5. Error Handling
*   **Crate**: `anyhow`
*   **Responsibility**: Propagating rich context errors to the UI logic for display in the Status Bar or Error Popups.
*   **Strategy**: 
    1.  **Friendly Hints**: A mapping engine in `helpers.rs` (`opc-da-client`) translates common technical codes (like licensing or RPC errors) into plain-English advice.
    2.  **Display Chain**: The UI uses `{:#}` formatting to show the full breadcrumb trail of a failure to the user.

## Application State Flow

The application follows a strict state machine to manage user context and navigation.

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

## Data Flow
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
        Tracing --> |Write| LogFile[opc-cli.log]
    end
```

## Branch Strategy & Release Workflow

To maintain a clean and pristine public-facing release history, the repository uses a divergent branch architecture:

*   **`dev` Branch**: The active development branch. All code changes, agent interactions, workflows (`.agents/`), and session logs (`context.md`) reside here.
*   **`main` Branch**: The production release branch. It contains only production source code and minimal tooling, completely clean of agent-related files, metadata, and dev-only rules.
*   **Release Merging (`Merge-ToMain.ps1`)**: Developers use the automated script to propagate changes from `dev` to `main`. Direct Git merges are prohibited, as the script is responsible for trimming out development assets and cleaning up `.gitignore` during the checkout and merge phases.

## Build System

The project uses a unified dual-interface build system:

1.  **Makefile**: The primary CLI entry point for developers. All complex multi-step workflows delegate directly to PowerShell scripts.
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

2.  **scripts/package.ps1**: Single PowerShell task dispatcher for all workspace operations.
    - Usage: `pwsh -File ./scripts/package.ps1 -Task <task>`
    - Supported tasks: `debug`, `release`, `build`, `test`, `verify`, `package`, `package-win7`, `logs`, `commit`, `release-merge`.

3.  **scripts/package-win7.ps1**: Dedicated legacy packaging pipeline that compiles polyfills, PE-patches the binary, and bundles redistributables.
4.  **scripts/verify.ps1**: Universal 5-gate quality pipeline (formatter, linter, doc-tests, workspace tests, polyfill compilation).
5.  **scripts/check-logs.ps1**: Log inspector and deep analysis utility.
6.  **scripts/commit.ps1**: Quality-gated commit & push pipeline.
7.  **scripts/Merge-ToMain.ps1**: Automated clean release merge tool.

## Legacy Compatibility (Windows 7 / Server 2008 R2)

Modern Rust binaries target Windows 8+ APIs by default. To support legacy NT 6.1 industrial environments (Windows 7 SP1 / Server 2008 R2 SP1), the repository implements a polyfill and binary-patching pipeline:

| Missing API on NT 6.1 | Polyfill / Patch Mechanism | Crate Source |
| :--- | :--- | :--- |
| `WaitOnAddress` / `WakeByAddressSingle` / `WakeByAddressAll` | 1ms polling `Sleep()` loop | `compat/synch-polyfill` (`api-ms-win-core-synch-l1-2-0.dll`) |
| `RoOriginateError` / WinRT Error APIs | No-op stub returning `S_OK`/`S_FALSE` | `compat/winrt-error-polyfill` (`api-ms-win-core-winrt-error-l1-1-0.dll`) |
| `ProcessPrng` | Routes to `RtlGenRandom` (`advapi32.dll`) | `compat/bcrypt-polyfill` (`bcryptprimitives.dll`) |
| `GetSystemTimePreciseAsFileTime` | Binary PE patch -> `GetSystemTimeAsFileTime` | `scripts/package-win7.ps1` inline byte replace |

### Standalone Crate Isolation
The polyfill crates in `compat/` are `#![no_std]` + `panic = "abort"` DLL projects. To avoid interfering with workspace quality gates (`cargo test --workspace`), these crates are **excluded** from the main Cargo workspace (`workspace.exclude = ["compat/*"]`). They are compiled independently by `scripts/package-win7.ps1` via `--manifest-path`.

## Testing Strategy

The project prioritizes a **Test-Driven Architecture** where the UI and business logic are decoupled from the underlying Windows COM/OPC dependencies.

### 1. Unit Testing (Mock-Based)
*   **Mechanism**: Uses the `mockall` crate to provide a `MockOpcProvider` during tests.
*   **Decoupling**: By abstracting OPC interactions behind the `OpcProvider` trait, the TUI and state transition logic can be verified on any platform (Linux/macOS/Windows) without a physical OPC server.
*   **Coverage** (80+ tests as of 2026-02-22):
    *   **UI Logic (`opc-cli/src/app.rs`)**: State transitions, navigation, search, tag selection, message ring-buffer, graceful timeout handling, and background task result polling.
    *   **Input Handling (`opc-cli/src/main.rs`)**: Key event processing across all screens.
    *   **OPC Logic (`opc-da-client`)**: HRESULT hint mapping, GUID filtering, FILETIME conversion, variant roundtrip, iterator bug detection, and `ComGuard` unit test.

### 2. Integration & Manual Testing
*   **OPC DA Layer (`opc-da-client`)**: Due to its direct reliance on Windows COM/DCOM, this layer is primarily verified through mock-backend integration tests and manual end-to-end testing against real OPC servers (e.g., Matrikon, Kepware, or local simulation servers).
*   **Async Boundaries**: Background task spawning and `tokio` timeouts are tested in `src/app.rs` using `#[tokio::test]`.
> [!IMPORTANT]
> Known bugs in the `opc_da` crate (like the **StringIterator E_POINTER flood**) and their workarounds are now documented in [opc-da-client/architecture.md](file:///c:/Users/WSALIGAN/code/opc-cli/opc-da-client/architecture.md).

## Design Principles
1.  **Testability First**: The UI should be verifiable without a running OPC server via mocks.
2.  **Robustness**: The app must not panic on missing COM servers; it should show error states in the UI.
3.  **Observability**: Since we cannot view stdout, file logging is mandatory for debugging.

