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
- **Startup Boundary Diagnostics**: Low-volume informational logs mark COM worker startup,
  ProgID resolution, server connection, capability detection, namespace organization, and the
  first native inventory operation, making startup stalls distinguishable from traversal stalls.
- **Scale-safe Iterator Diagnostics**: Native iterator refill timing and per-entry null handling
  are trace-level diagnostics, so production-scale inventories do not fill debug logs with
  routine enumeration events.
- **Backend-aware Inventory Pacing**: Charges DA3 pages by requested page size and DA2 string enumeration by actual native cache refills, so cached items do not add artificial delays.
- **Bounded Browse Safety**: Native and compatibility browse iterators terminate with a contextual error after 64 consecutive identical successful values, preventing a non-progressing OPC enumerator from running indefinitely.
- **Continuation-safe Inventory Traversal**: Internal DA3 inventory traversal rejects missing,
  empty, repeated, and cyclic continuation tokens, and stops after 64 consecutive empty
  continuation pages with a typed non-progress error. Finite temporary empty pages remain
  valid; public `browse_page` calls remain one-page operations under caller control.
- **DA2 Branch Recovery**: During hierarchical inventory, a non-progressing branch iterator is discarded so the independent item iterator can continue; item-side non-progress and unrelated errors remain terminal.
- **Failure-safe Inventory Worker**: Converts worker panics and inventory errors into terminal stream errors instead of silently ending the stream.
- **Cancellation Diagnostics**: Inventory cancellation logs identify the requesting source and whether cancellation was already pending, distinguishing explicit cancellation from stream-drop cleanup.
- **Defensive COM Iterators**: Rejects native enumerator counts that exceed the fixed cache capacity before indexing the returned buffer, bounds null-only batches, and releases every remaining COM-allocated string after failed or early-ended iteration.
- **Windows COM/DCOM Support**: Native OPC DA backend via `windows-rs` — no external OPC crates needed.
- **Robust Error Handling**: Leverages `thiserror` for the `OpcError` domain type and `friendly_com_hint()` for human-readable HRESULT explanations.
- **Test-Friendly**: Built-in `MockOpcProvider` via the `test-support` feature.

## Installation

Add this to your `Cargo.toml`:

```toml
[dependencies]
opc-da-client = { package = "bytehound-opc-da-client", version = "0.2.8" }
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

The repeated-value safety bound applies to each underlying native iterator, not
to the requested page size. A page smaller than the 64-value bound can therefore
return a continuation before the iterator guard is reached; continue paging to
consume the full bounded iterator.

`start_inventory` applies an additional progress guard to its private DA3
continuation loop. A response that reports more elements must provide a
non-empty token that has not already been returned for that branch; repeated
tokens, including cycles, terminate the inventory with a contextual error.
Empty pages are allowed when they are temporary, but 64 consecutive empty
continuation pages are treated as non-progress. This guard belongs only to the
internal full-inventory worker. The public `browse_page` API returns exactly
one bounded page and leaves continuation handling to the caller.

The DA 2.x fallback merges a same-named branch and leaf into one
`BrowseNodeKind::BranchAndItem` node and resolves its exact item ID through
`GetItemID`.

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
dynamically set the minimum interval between native operation starts and the
maximum requested item rate. DA3 page operations charge the requested page size
before each native call, even when the server returns fewer entries. Hierarchical
DA2 string enumeration charges the actual `IEnumString::Next` refill size
(currently the native iterator cache capacity); items already returned from
that cache do not consume another pacing budget. Cancellation is still checked
before each cached item as well as before each refill.
Use `InventoryStream::set_batch_size(batch_size)` to change the bounded request
size before the next slice; values must be between 1 and
`MAX_INVENTORY_BATCH_SIZE` (1000).
Each completed slice emits an `InventoryEvent::Slice` observation with its
backend, result count, operation count, and cumulative progress totals.
For DA2 hierarchical namespaces, every server-reported branch is validated with
a bounded native navigation probe. Branch-only names rejected with
`E_INVALIDARG` are skipped and reported in the inventory completion warning;
names that resolve to exact items remain selectable even when they are not
navigable. If the DA2 branch iterator itself reaches the non-progress threshold,
only that iterator is discarded and item enumeration continues. The completion
warning identifies the skipped branch iterator; non-progressing item iterators
and unrelated native errors remain terminal.
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
