# Architecture: bytehound-opc-da-client

## Public API and platforms

The package is `bytehound-opc-da-client`; its Rust library name is
`opc_da_client`. The manifest defines the version, Rust 2024 edition, and
Rust 1.88 MSRV. The behavioral contract is in [spec.md](spec.md).
Code and tests are authoritative.

The public provider trait, value/error models, opaque browse tokens, inventory
controls and streams, and operation telemetry models compile on Windows and
Linux. `MockOpcProvider` is available with `test-support`. This is a real
model/provider layer, not an empty non-Windows crate.

The `opc-da-backend` default feature enables the native implementation on
Windows. COM dependencies, native groups and handles, `OpcDaClient`, and
`com_worker` are Windows-only. `ComGuard` and HRESULT formatting are also
Windows-only. Disabling default features selects the portable provider/model
layer; it does not provide a non-Windows COM implementation.

## Source layout

| Path under `src/` | Responsibility |
| --- | --- |
| `lib.rs` | Platform gates and stable public re-exports |
| `provider.rs` | Provider trait, public data models, controls, streams, histograms, and mock generation |
| `errors.rs` | Portable errors, Windows COM source conversion, hints, and structured error logging |
| `com_guard.rs` | Caller-thread and worker-thread MTA initialization guard |
| `com_worker.rs` | Worker lifecycle, bounded channel, request dispatch, and result delivery |
| `com_worker/request.rs` | Public `ComRequest` and `ReadPresentation`, re-exported through `com_worker` |
| `com_worker/connection.rs` | Connection cache and classified reconnect/retry |
| `com_worker/read.rs`, `write.rs`, `browse.rs` | Native operations and existing cleanup/navigation paths |
| `inventory.rs` | Independent-connection traversal orchestration |
| `inventory/state.rs` | Branch work, exact locations, and private continuation state |
| `inventory/boundary.rs` | Pause, cancellation, operation-start pacing, and accounting |
| `inventory/telemetry.rs` | Nested thread-local collectors and first-seen operation aggregation |
| `inventory/error.rs` | Portable cancellation and contextual traversal errors |
| `inventory/capabilities.rs`, `progress.rs` | Capability detection, progress, warnings, and event delivery |
| `inventory/da3.rs` | DA3 mapping and branch-local continuation progress |
| `inventory/da2.rs`, `iterator.rs`, `navigation.rs` | Deferred expansion, buffered enumeration, and exact browse-to/fallback |
| `native_browse.rs` | Session lifecycle and one-level page dispatch |
| `native_browse/state.rs`, `capabilities.rs`, `da2.rs`, `da3.rs` | Token bounds, namespace detection, and protocol-specific pages |
| `backend/connector.rs`, `backend/opc_da.rs` | Native connector abstractions and provider implementation |
| `helpers.rs` | Windows VARIANT, quality, timestamp, and ProgID conversion |
| `opc_da/` | Internal native client, traits, typedefs, COM memory wrappers, and error compatibility re-exports |
| `bindings/` | Generated OPC DA and common COM interface bindings |
| `tests/` | Shared tracing support and domain characterization tests |

The boundary, error, and telemetry modules contain no COM types and are exercised
off Windows. Native traversal and session state remain private to their owning
workers. The crate package includes this directory, the behavioral contract, all
source modules, and public-model integration tests.

## COM ownership and lifetime

`ComWorker::start` creates a dedicated thread and initializes COM in
Multi-Threaded Apartment (MTA) mode. The apartment model is not STA.
The worker declares its `ComGuard` before the connection cache and browse
sessions, so those resources are destroyed before COM teardown. Native server,
group, and enumerator operations and destruction stay on that worker.

Requests cross a Tokio MPSC channel with capacity 32; replies use one-shot
channels. Request payloads contain owned Rust values and opaque tokens, not COM
pointers. Read, write, capability, and recursive browse requests share a
ProgID-keyed connection cache. Only classified disconnect/server-start HRESULTs
evict a cached connection and trigger one retry.

Each interactive browse session owns a separate server connection and mutable
DA2 position. Inventory uses its own worker and connection, separate from
foreground reads and interactive sessions. Its event channel has capacity 64.
`InventoryStream::drop` closes the receiver before cancellation and joining, so
backpressure cannot leave the worker blocked on an abandoned receiver.
`ComWorker::drop` signals shutdown through channel ownership; it does not add a
synchronous join.

`ComGuard` is neither `Send` nor `Sync`. A caller doing additional COM work holds
its own guard on that caller thread. Each unsafe operation needs a local
`SAFETY` explanation of its pointer validity, allocation/ownership, or ABI
invariant. Test allocations check non-null pointers before initialization and
transfer ownership to the corresponding COM-memory wrapper.

## Browsing and identity

Three surfaces have distinct responsibilities:

1. `browse_tags` is the bounded recursive compatibility interface.
2. Native session-backed `browse_page` returns one bounded level.
3. Inventory streams exact selectable ItemIDs and traversal diagnostics.

Names and ItemIDs are never split on `.`, `!`, or `/` to infer hierarchy.
Session, node, and continuation tokens are opaque UUIDs. Raw DA3 continuations,
DA2 paths, and enumerators remain worker-owned.

DA3 is preferred when available. The first actual server-root page may fall
back to DA2 only for `RPC_X_NULL_REF_POINTER` or `E_NOTIMPL` and only when DA2
is available. A successful interactive root page locks that session to DA3.
A failed explicitly rooted inventory cannot turn into full-server fallback.

`IOPCBrowse::Browse` declares `pdwPropertyIDs` as a top-level
`[in, size_is(dwPropertyCount)] DWORD*` parameter. MIDL's
[`pointer_default(unique)`](https://learn.microsoft.com/en-us/windows/win32/midl/pointer-default)
does not apply to top-level parameters, so this is a reference pointer and must
be non-null even at count zero. The empty-property path supplies a live
placeholder address with a zero count; `size_is(0)` sends no property-ID
elements. The Windows RPC/NDR fixture compiles a matching MIDL parameter shape
and checks both null-pointer rejection and successful zero-length marshalling
over an out-of-process `ncalrpc` call. It does not activate an OPC server or
use a live gateway.

Interactive DA2 pages classify immediate branches/items, merge same-named
branch-and-item nodes, and isolate each session's cursor. Hierarchical inventory
instead defers branch expansion and avoids eager child/classification probes.
It attempts canonical `OPC_BROWSE_TO` when an exact ItemID is known and falls
back to component navigation only for classified compatibility errors.
Recoverable branch-side non-progress or deferred navigation rejection does not
discard independent item enumeration; item-side non-progress stays terminal.

Inventory validates DA3 continuation uniqueness and bounds consecutive empty
pages. Interactive `browse_page` remains caller-driven and does not drain
continuations or apply the inventory-only progress guard.

## Pacing and telemetry

Cancellation and pause are checked at bounded operation boundaries, including
between cached DA2 entries. They do not interrupt a COM call already in flight.
Pacing updates are observed without restarting inventory.

The minimum interval between native operation starts and the requested item-rate
cap are independent controls. DA3 charges the requested page size. DA2 charges
actual native `IEnumString::Next` refills at cache capacity; cached values do not
consume another native pacing budget.

Telemetry scopes are nested and thread-local. Each collector preserves
first-seen operation-kind order and stores saturated counts/durations, inclusive
fixed nanosecond histogram buckets, and approximate p50/p95/p99 values.
Pacing waits are excluded from native elapsed time; an entered call is counted
even if it fails. Startup observations are separate from traversal slices.

The collector contains only best-effort numeric telemetry, not native resources.
Poison recovery preserves available samples and prevents an accounting failure
from becoming an inventory failure. It does not alter cancellation, traversal,
or COM ownership.

## Errors, values, and observability

Fallible operations return `OpcResult<T>`. On Windows,
`OpcError::Com { source }` retains the native source chain, HRESULT, and friendly
display hint. Contextual browse errors retain escaped path/item diagnostics;
typed non-progress errors keep their counters and repeated values.

Semantic reads preserve BSTR contents exactly. Display reads add presentation
quotes only in the native provider. Quality classification and local FILETIME
formatting belong to the Windows conversion layer. A rejected per-tag write is
reported through `WriteResult`, not silently treated as success.

`tracing` emits structured events without installing a subscriber. Consumers
choose output destinations. Extracted inventory, native-browse, COM-worker, and
error helpers retain their established tracing targets. Startup milestones use
`info`; normal operation completions use `debug`; per-refill detail uses `trace`;
failures and relevant recovery transitions use `warn` or `error`.

## Validation

Run commands from the workspace root. See
[CONTRIBUTING.md](https://github.com/bytehound-labs/opc-cli/blob/main/CONTRIBUTING.md)
and
[scripts/verify.ps1](https://github.com/bytehound-labs/opc-cli/blob/main/scripts/verify.ps1).

```sh
cargo fmt --all -- --check
cargo clippy --locked -p bytehound-opc-da-client --all-targets --all-features -- -D warnings
cargo test --locked -p bytehound-opc-da-client --all-features
cargo clippy --locked -p bytehound-opc-da-client --all-targets --no-default-features -- -D warnings
cargo test --locked -p bytehound-opc-da-client --no-default-features
pwsh -File opc-da-client/tests/browse_ndr/verify.ps1
npx --yes -p @ast-grep/cli@0.45.3 ast-grep test
npx --yes -p @ast-grep/cli@0.45.3 ast-grep scan
```

The Windows RPC/NDR fixture requires the Windows SDK MIDL compiler and Visual
Studio C++ build tools. It selects AMD64 with MIDL's
[`/env amd64`](https://learn.microsoft.com/en-us/windows/win32/midl/-env),
matches the client/server routine prefixes, and links the generated RPC stubs
directly; registration uses the prefixed RPC server interface handle declared
in the generated header. A plain RPC interface has no COM IID source file. It
uses a local RPC endpoint and does not require OPC Core Components or a live
OPC server. Both processes launch through `System.Diagnostics.Process`, retaining
their startup handles for exit-status checks on Windows PowerShell 5.1. The
client completion and server shutdown waits remain bounded to 15 and 10 seconds,
respectively.

Linux tests cover real public models, provider defaults and mocks, stream cleanup,
thread-local telemetry, cancellation/pacing, and poison recovery. Windows CI also
runs workspace Clippy/tests/doctests, model-only checks, 32-bit Clippy, package
verification, and the Rust 1.88 MSRV check.

Mock-native tests cover protocol mapping, exact IDs, cancellation, cursor
ownership, connection retry, and worker-thread destruction. They do not prove
live OPC/DCOM interoperability. The ignored live-server cursor probe is not part
of automated acceptance and requires separate authorization.

## Dependencies and publication

Portable dependencies provide Rust errors, futures/channels, tracing, and UUID
models. Windows-target dependencies provide COM bindings and local timestamp
conversion. `mockall` is optional test support. Exact dependency selections are
recorded in the manifests and lockfile.

Contributions use protected feature-branch PRs and squash merges, not the legacy
dev-to-main scripts. Publication is a separately authorized workflow documented
in [publishing.md](https://github.com/bytehound-labs/opc-cli/blob/main/docs/publishing.md).
CI verifies package contents with `cargo package`; this does not upload a crate
or authorize creating a release.
