# bytehound-opc-da-client

[![Crates.io](https://img.shields.io/crates/v/bytehound-opc-da-client.svg)](https://crates.io/crates/bytehound-opc-da-client)
[![Docs.rs](https://docs.rs/bytehound-opc-da-client/badge.svg)](https://docs.rs/bytehound-opc-da-client)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

Backend-agnostic OPC DA client library for Rust — async, trait-based, with transparent COM management.

## Features

- **Async/Await API**: Built for modern asynchronous Rust using `tokio` and `async-trait`.
- **Trait-Based Abstraction**: The `OpcProvider` trait allows for easy mocking and backend swapping.
- **Transparent COM Management**: Handles COM initialization (`CoInitializeEx`) and apartment thread affinity automatically in the background.
- **Read & Write Support**: Read tag values and write typed values (`Int`, `Float`, `Bool`, `String`) to OPC tags.
- **Scalable Native Browsing**: Open isolated sessions and request bounded, one-level pages through OPC DA 3.0, with a narrowly negotiated OPC DA 2.x compatibility fallback.
- **Bounded Namespace Inventory**: Stream exact ItemIDs with breadcrumb labels through a cancellable, bounded DA 3.0/2.x traversal.
- **Failure-safe Inventory Worker**: Converts worker panics and inventory errors into terminal stream errors instead of silently ending the stream.
- **Defensive COM Iterators**: Rejects native enumerator counts that exceed the fixed cache capacity before indexing the returned buffer.
- **Windows COM/DCOM Support**: Native OPC DA backend via `windows-rs` — no external OPC crates needed.
- **Deterministic COM Ownership**: Native arrays have one owner and are converted without shallow pointer clones; nested strings, blobs, item-state values, and property values are released on the COM worker thread after conversion, including error and partial-result paths. Returned element counts are checked against physical COM allocation bounds, and failed property entries are still cleared according to the OPC DA contract.
- **Owned VQT Writes**: OPC DA value-quality-timestamp writes use a non-clonable owning wrapper for each copied `VARIANT`, which clears the value after the COM call instead of relying on the generated shallow VQT clone.
- **Robust Error Handling**: Leverages `thiserror` for the `OpcError` domain type and `friendly_com_hint()` for human-readable HRESULT explanations.
- **Test-Friendly**: Built-in `MockOpcProvider` via the `test-support` feature.
- **Opt-in Native Diagnostics**: The `dev-diagnostics` feature exposes a read-only native canary and a JSON Lines example for comparing direct COM reads with the normal worker path.

## Installation

Add this to your `Cargo.toml`:

```toml
[dependencies]
opc-da-client = { package = "bytehound-opc-da-client", version = "0.2.5" }
```

## Prerequisites

- **Operating System**: Windows (COM/DCOM is a Windows-only technology).
- **Rust**: 1.88 or newer.
- **OPC DA Core Components**: Ensure the OPC DA Core Components are installed and registered on your system.
- **DCOM Configuration**: If connecting to remote servers, appropriate DCOM permissions must be configured.

## Usage Examples

### Connecting & Listing Servers

Enumerate available OPC DA servers on a local or remote host.

```rust,no_run
use opc_da_client::{OpcDaClient, OpcProvider};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let client = OpcDaClient::default();

    let servers = client.list_servers("localhost").await?;
    println!("Available Servers:");
    for server in servers {
        println!("  - {}", server);
    }
    Ok(())
}
```

### Reading Tags

Connect to a specific server and read current values for a set of tags.

```rust,no_run
use opc_da_client::{OpcDaClient, OpcProvider};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let client = OpcDaClient::default();
    let server_progid = "Matrikon.OPC.Simulation.1";
    let tags = vec![
        "Random.Int4".to_string(),
        "Random.Real8".to_string(),
    ];

    let values = client.read_tag_values(server_progid, tags).await?;

    for v in values {
        println!("Tag: {}, Value: {}, Quality: {}, Time: {}",
            v.tag_id, v.value, v.quality, v.timestamp);
    }
    Ok(())
}
```

`read_tag_values` is the machine-facing read API. For `VT_BSTR` values, `TagValue::value`
contains the exact COM string contents: no quote characters are added or removed. Consumers
that intentionally want the historical quoted string presentation can call
`read_tag_values_for_display`; its default trait implementation falls back to
`read_tag_values` for third-party providers.

### Native Read-Only Canary

Enable `dev-diagnostics` only for troubleshooting native OPC DA behavior. The feature adds
`opc_da_client::diagnostics` and the `native_read_canary` example; its `serde` and
`serde_json` dependencies are not part of normal builds.

```powershell
cargo run -p bytehound-opc-da-client `
  --example native_read_canary `
  --features dev-diagnostics -- `
  Yokogawa.CSHIS_OPC.1 `
  FCS0201!204FI00510.PV `
  FCS0201!204FI00510.OUT `
  --update-rate-ms 1000
```

The command is non-interactive and writes one JSON object per line to stdout. It connects
directly by ProgID, records server status and locale information, asks the server for its
text for HRESULT `0xC004800B`, validates and adds the exact ItemIDs to a temporary active
group, records canonical type/access metadata and available standard item properties, and
performs explicit device and cache reads immediately and after one and two server-revised
update intervals. Each per-item result includes the HRESULT in hexadecimal plus Windows and
vendor text, and successful reads include safely formatted value, quality, and timestamp
fields. The temporary group is removed before the normal `OpcDaClient` worker reads one
selected item for comparison.

The canary is strictly read-only: it never calls OPC write APIs, never changes the requested
ItemIDs, and never substitutes a cache read when a device read fails. Device and cache
observations are independent records. Library callers can use
`run_native_read_canary(NativeReadCanaryConfig)` and `write_json_lines` directly.

### Bounded Native Inventory Diagnostics

The same diagnostic example can run a bounded namespace inventory without SQLite or gateway
persistence:

```powershell
cargo run -p bytehound-opc-da-client `
  --example native_read_canary `
  --features dev-diagnostics -- `
  inventory Yokogawa.CSHIS_OPC.1 `
  --start-path FCS0219 `
  --start-path 203FI02005 `
  --batch-size 25 `
  --max-entries 1000 `
  --min-interval-ms 25 `
  --deadline-secs 60
```

The inventory command emits JSON Lines for `inventory_start`, each `entry`, `progress`, and
`slice`, followed by `completed` and a final `inventory_result`. Display names, ItemIDs, and
breadcrumbs are serialized with normal JSON escaping, including control characters. The
default configuration is intentionally conservative: a batch size of 25, a 100-entry limit,
25 ms minimum operation interval, and a 60-second deadline. `--item-rate-per-second` adds an
independent requested item-rate limit. Repeated `--start-path <COMPONENT>` values select a
diagnostic-only DA2 browse path instead of the namespace root. The `inventory_start` record
contains the selected path, and entries below the path retain their normal breadcrumbs and
exact ItemIDs. The path mode is useful for proving whether a known nested branch is reachable
without waiting for a breadth-first root traversal to reach it; it does not change the stable
`OpcProvider::start_inventory` API or production inventory behavior. A server that does not
support DA2 rejects a targeted path explicitly.

Use `--cancel-after-secs <N>` for an explicit cancellation run. Cancellation requests are
reported before the worker reaches its next bounded native operation. A deadline emits
`deadline_expired`, requests cancellation, and allows a five-second grace period before a
blocked native worker is detached. `Completed`, stream errors, and channel closure are joined
directly so a normal terminal event cannot be mistaken for a still-running worker. The final
result classifies the run as `completed`, `stream_error`, `channel_eof`, `deadline`, or
`worker_failure`; only `completed` is a successful diagnostic result.


### Writing a Value

Write a typed value to a single OPC tag.

```rust,no_run
use opc_da_client::{OpcDaClient, OpcProvider, OpcValue};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let client = OpcDaClient::default();
    let server = "Matrikon.OPC.Simulation.1";

    let result = client
        .write_tag_value(server, "Bucket Brigade.Int4", OpcValue::Int(42))
        .await?;

    if result.success {
        println!("✓ Write succeeded");
    } else {
        println!("✗ Write failed: {}", result.error.as_deref().unwrap_or("Unknown error"));
    }
    Ok(())
}
```

### Browsing the Address Space

Recursively discover available tags on an OPC server.

```rust,no_run
use opc_da_client::{OpcDaClient, OpcProvider};
use std::sync::{Arc, Mutex, atomic::AtomicUsize};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let client = OpcDaClient::default();
    let server_progid = "Matrikon.OPC.Simulation.1";

    let sink = Arc::new(Mutex::new(Vec::new()));
    let progress = Arc::new(AtomicUsize::new(0));
    // Clone these Arcs before passing if you need to monitor progress
    // or harvest partial results from another task on timeout.

    let discovered_tags = client.browse_tags(
        server_progid,
        100, // Max tags to discover
        progress,
        sink
    ).await?;

    println!("Found {} tags", discovered_tags.len());
    Ok(())
}
```

For large namespaces, use the bounded native browse API instead of recursive discovery:

```rust,no_run
use opc_da_client::{
    BrowseNodeFilter, BrowsePageRequest, OpcDaClient, OpcProvider,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let client = OpcDaClient::default();
    let server = "Matrikon.OPC.Simulation.1";
    let capabilities = client.browse_capabilities(server).await?;
    let session = client.open_browse_session(server).await?;

    let page = client
        .browse_page(
            &session,
            BrowsePageRequest {
                parent: None,
                filter: BrowseNodeFilter::All,
                max_elements: capabilities.max_page_size.min(100),
                continuation: None,
            },
        )
        .await?;

    for node in page.nodes {
        println!("{}: {:?}", node.name, node.kind);
    }
    client.close_browse_session(&session).await?;
    Ok(())
}
```

Session, node, and continuation tokens are opaque UUIDs. Native browse sessions
own dedicated server connections, expire after five minutes of inactivity, and
never expose COM pointers or OPC DA continuation strings. Transport adapters can
encode tokens with `to_string()` and restore them with each token type's
`parse()` method.

The DA 2.x fallback merges a same-named branch and leaf into one
`BrowseNodeKind::BranchAndItem` node and resolves its exact item ID through
`GetItemID`.

The diagnostic-only `native_read_canary browse` mode can inspect a server-returned
branch and its immediate children without writing values or using SQLite:

```text
native_read_canary browse Yokogawa.CSHIS_OPC.1 --path SCS0130 --page-size 250
```

It emits JSON Lines containing the server's exact child names, item IDs, node kinds,
and browse continuation tokens. This is intended for troubleshooting namespace
formation; use returned item IDs for subsequent read tests rather than constructing
ItemIDs from project files.

For both DA 3.0 and DA 2.x, only selectable `Item` and `BranchAndItem`
nodes expose `item_id`. Branch-only nodes retain any native ItemID needed for
child navigation inside the session and return `item_id: None` to callers.

The first root page is also the DA 3.0 compatibility check. Required root and
unused-filter arguments are sent as non-null empty UTF-16 strings, as specified
by OPC DA. The initial continuation is a non-null outer pointer containing a
null inner pointer, and an empty property-ID list is sent as a null pointer.
If that first call still returns `RPC_X_NULL_REF_POINTER` or
`E_NOTIMPL` and the server exposes DA 2.x browsing, the session logs the
compatibility failure and continues through DA 2.x. Access, transport,
disconnect, timeout, and other COM failures remain visible and never trigger a
fallback. After the first DA 3.0 root page succeeds, the session remains on DA
3.0 so existing node and continuation tokens cannot be mixed with DA 2.x state.

For large namespaces, `start_inventory` streams a bounded inventory without
persisting browse-session or continuation tokens:

```rust,no_run
use opc_da_client::{
    InventoryEvent, InventoryOptions, InventoryPacing, OpcDaClient, OpcProvider,
};
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let client = OpcDaClient::default();
    let mut inventory = client
        .start_inventory(
            "Matrikon.OPC.Simulation.1",
            InventoryOptions {
                batch_size: 100,
                max_entries: None,
            },
        )
        .await?;
    inventory.set_pacing(InventoryPacing {
        min_interval: Duration::from_millis(25),
        item_rate_per_second: Some(50),
    });
    inventory.set_batch_size(50)?;

    while let Some(event) = inventory.message().await {
        match event? {
            InventoryEvent::Entry(entry) => println!("{}: {}", entry.display_name, entry.item_id),
            InventoryEvent::Slice(slice) => {
                println!("slice {}: {} native operations", slice.sequence, slice.native_operations);
            }
            InventoryEvent::Progress(progress) => {
                println!("{} items discovered", progress.unique_items);
            }
            InventoryEvent::Completed(result) => {
                println!("complete: {}", result.complete);
                break;
            }
        }
    }
    Ok(())
}
```

The returned `InventoryStream` exposes pause, resume, and cancellation controls.
Each native browse call is bounded by `InventoryOptions::batch_size`, and
`max_entries` can cap a deliberately limited inventory.
Use `InventoryStream::set_pacing(InventoryPacing { min_interval, item_rate_per_second })` to
dynamically set the minimum interval between bounded native operation starts
and the maximum requested item rate. The item-rate budget is charged for the
requested batch size before each native call, even when the server returns
fewer entries.
Use `InventoryStream::set_batch_size(batch_size)` to change the bounded request
size before the next slice; values must be between 1 and
`MAX_INVENTORY_BATCH_SIZE` (1000).
Each completed slice emits an `InventoryEvent::Slice` observation with its
backend, result count, operation count, and cumulative progress totals.
For DA2 hierarchical namespaces, every server-reported branch is validated with
a bounded native navigation probe. Branch-only names rejected with
`E_INVALIDARG` are skipped and reported in the inventory completion warning;
names that resolve to exact items remain selectable even when they are not
navigable. Other COM and transport failures remain visible errors.
Inventory uses the same first-root-page DA 3.0 negotiation as interactive
browsing and reports DA 2.x as its source when compatibility fallback is used.
Completion warnings are cumulative, so an entry limit or skipped branch does
not erase the compatibility diagnostic.

## Architecture

The library is split into a core trait layer and concrete implementations:

- **`OpcProvider`**: The primary async trait defining server discovery, recursive tag browsing, native paged browsing, reads, and writes.
- **`OpcDaClient`**: The default implementation using native `windows-rs` COM calls. Generic over `ServerConnector` for testability; defaults to `ComConnector`.

See [architecture.md](https://github.com/bytehound-labs/opc-cli/blob/main/opc-da-client/architecture.md) for in-depth design details and [spec.md](https://github.com/bytehound-labs/opc-cli/blob/main/opc-da-client/spec.md) for behavioral contracts.

### COM Threading Model

OPC DA relies on Windows COM, which requires per-thread initialization and strict thread affinity. The `opc-da-client` dependency alias handles this transparently:
* **Dedicated Worker Thread**: All COM operations are executed on a dedicated background worker thread initialized in Multi-Threaded Apartment (MTA) mode.
* **No Manual Init**: You do not need to call `CoInitialize` or manage COM lifecycles in your calling application.
* **Host Thread Initialization**: Applications that also perform COM work on their own thread can hold a public `ComGuard::new()` guard for that thread's lifetime.

## License

This project is licensed under the MIT License.
