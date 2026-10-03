# Behavioral contract: bytehound-opc-da-client

This document describes the provider, native worker, browse, inventory, and
conversion contracts. Code and tests are authoritative. Module ownership and
validation commands are in [architecture.md](architecture.md).

## Provider and public models

`OpcProvider: Send + Sync` uses `async-trait` and `OpcResult<T>`.

| Method | Contract |
| --- | --- |
| `list_servers(host)` | Enumerate server ProgIDs through the provider; the native connector enumerates the local Windows registration |
| `browse_tags(server, max_tags, progress, tags_sink)` | Bounded recursive compatibility discovery with incremental progress and sink updates |
| `browse_capabilities(server)` | Report namespace organization, DA2/DA3 support, and maximum page size |
| `open_browse_session(server)` | Open an isolated worker-owned native connection and return an opaque token |
| `browse_page(session, request)` | Return one bounded, non-recursive level |
| `close_browse_session(session)` | Release that session's connection, nodes, and continuations |
| `start_inventory(server, options)` | Stream a cancellable, bounded full-server inventory on an independent connection |
| `start_inventory_at_root(server, root_item_id, options)` | Preserve one exact canonical root ItemID without inferring components |
| `read_tag_values(server, tag_ids)` | Preserve semantic values, including exact BSTR contents |
| `read_tag_values_for_display(server, tag_ids)` | Delegate to semantic reads by default; the native provider adds BSTR presentation quotes |
| `write_tag_value(server, tag_id, value)` | Write one typed value and return per-tag acceptance/rejection in `WriteResult` |

Native browse and inventory trait methods default to `OpcError::NotImplemented`
for providers that do not implement those capabilities. Adding native
capabilities does not require third-party providers to override the defaults.

`TagValue` keeps the exact tag ID and string value, quality, and timestamp.
`OpcValue` variants are `String`, `Int(i32)`, `Float(f64)`, and `Bool`.
`WriteResult` keeps `tag_id`, `success`, and an optional error string.
Models do not invent controller/tag namespaces or reinterpret stored strings.

`BrowseSessionToken`, `BrowseNodeToken`, and `BrowsePageToken` encode/parse opaque
UUIDs. They do not encode ItemIDs or public COM pointers.
`BrowseNodeKind::BranchAndItem` is both expandable and selectable.
Branch-only nodes do not expose a selectable ItemID.

The trait, models, controls/streams, errors, and mock provider are portable.
COM-specific variants/types and native methods require Windows. Crate-root
paths such as `OpcError`, `OpcResult`, and the provider models remain stable.
Windows consumers retain
`com_worker::{ComWorker, ComRequest, ReadPresentation}`.

## Worker and resource ownership

- The dedicated native worker initializes MTA, not STA.
- Its COM guard is declared before its connection cache and browse sessions.
  Cached servers and session resources are dropped before the guard.
- Native server/group/enumerator operations and destruction stay on the owner
  worker; channel requests contain owned Rust values and opaque tokens.
- Cached operations retry once only after classified connection errors:
  `0x800706BA`, `0x800706BF`, `0x800706BE`, or `0x80080005`.
- An interactive session has its own server connection and mutable DA2 position.
- Inventory has a separate worker/connection from foreground operations.
- Cancelled open/page replies avoid or close the associated session.
- Closing, expiration, or worker shutdown releases session state on the worker.
- Dropping an inventory stream closes its receiver before joining its worker;
  normal terminal completion does not add a false cancellation.
- Dropping `ComWorker` signals channel shutdown without a synchronous join.
- `ComGuard` must remain on the initializing thread and is neither `Send` nor
  `Sync`. Successful `CoInitializeEx`, including `S_FALSE`, is balanced by
  `CoUninitialize`.

## Native paged browsing

Page sizes are from 1 through 1,000. There are at most 64 open sessions.
Sessions expire after five minutes idle, and each session holds at most 100,000
node tokens and 256 continuation tokens.

Parent tokens must belong to the session and identify a node with children.
Flat namespaces reject a non-root parent. A continuation must match the requested
parent and filter, and is consumed once. Invalid, closed, expired, cross-session,
or already consumed tokens return errors.

### DA3

- Native branch/item flags map to `Branch`, `Item`, or `BranchAndItem`.
  An element with neither flag is invalid.
- Selectable items require an exact ItemID. Branch navigation keeps the
  server-supplied ItemID private when the branch is not selectable.
- Root/filter strings are non-null empty UTF-16 strings. Initial continuation
  storage is non-null and contains a null value.
- The zero-property path preserves the established null property-ID pointer;
  nonempty property lists use the generated binding. ABI changes require their
  own native compatibility validation.
- Only the first actual server-root page may fall back to DA2, for
  `RPC_X_NULL_REF_POINTER` or `E_NOTIMPL`, when DA2 is available.
  Other errors stay terminal. A successful root page locks the session to DA3.
- A `more_elements` response requires a continuation. Public paging does not
  drain continuations or apply inventory's repeated/empty-page guard.

### DA2

- Hierarchical pages enumerate immediate branches and leaves only.
  Same-named branch/leaf entries merge into one `BranchAndItem`, including
  across page boundaries.
- Selectable ItemIDs come from `GetItemID`, not separator parsing.
- Flat paging uses `OPC_FLAT` without child recursion.
- Session cursors are isolated. Navigation to another path uses the common
  ancestor and exact server-returned components.
- Recoverable branch-side non-progress discards that iterator and allows
  independent item enumeration; item-side non-progress remains terminal.

The recursive compatibility API stays separate: it caps `max_tags`, uses a
maximum depth of 50, enumerates branch names before leaves, and emits the
current level's leaves before recursing into those branches. It attempts `UP`
even when recursion fails. A failed `UP` is logged and stops the remaining
branch recursion; it does not become a new returned error.
Hierarchical discovery does not treat `OPC_FLAT` output as complete ItemIDs.

## Inventory

`InventoryOptions` defaults to batch size 100 and no entry cap. Accepted batch
sizes are from 1 through 1,000. The worker emits `Entry`, `Progress`, `Slice`, and
`Completed` events; errors are terminal stream errors.

- Every selectable entry retains the exact ItemID and breadcrumb labels.
  Duplicate ItemIDs are emitted once without discarding child traversal.
- A root-scoped request retains its canonical root unchanged.
  DA3 compatibility fallback is allowed only for the true server root.
- Hierarchical DA2 expansion is deferred until a branch is visited.
  Inventory does not eagerly probe item children or classify branches with
  `DOWN`/`UP`.
- `GetItemID` detects same-named branch-and-item nodes and supplies a canonical
  `OPC_BROWSE_TO` target.
- Browse-to fallback is limited to `NotImplemented`, `E_INVALIDARG`, `E_NOTIMPL`,
  `RPC_X_NULL_REF_POINTER`, `OPC_E_UNKNOWNITEMID`, or `OPC_E_INVALIDITEMID`.
  Other direct-navigation errors are terminal.
- A rejected deferred compatibility navigation or branch-side non-progress is
  skipped with a cumulative warning, while independent items continue.
  Unrelated errors and item-side non-progress remain terminal.
- Each DA3 `more_elements` response needs a nonempty, previously unseen token
  for that branch. Repeated tokens and cycles are terminal.
  Fewer than 64 consecutive empty continuation pages are allowed; reaching
  64 is typed continuation non-progress.
- Cancellation is checked before bounded operations and between cached DA2
  items. Pause and pacing changes apply without restarting the worker.
  An in-flight COM call is not forcibly interrupted.
- Cancelled/truncated inventory does not claim `complete = true`.
  Fallback, truncation, and malformed-branch warnings are merged.
- Worker panics and traversal failures are delivered as terminal errors rather
  than silently ending the stream.

### Pacing

The minimum native-start interval and requested item-rate cap are separate.
DA3 charges requested page size. DA2 charges actual native cache refills at
cache capacity; cached entries do not add pacing charges. A zero item-rate
normalizes to no rate cap; it is not a pause request.

### Telemetry

Each slice stores sequence, backend, node count, continuation status, bounded
operation count, wall elapsed time, cumulative entry counts, and typed native
observations. Observations preserve first-seen kind order and saturated numeric
counts/durations, histogram buckets, and approximate p50/p95/p99 values.
Histogram bounds are inclusive; the overflow bucket uses observed maximum time.

Native elapsed time excludes pacing waits. Entered failures are counted;
cancellation before entry is not. Startup capability observations are separate
from the first traversal slice.

Nested thread-local scopes restore the previous collector and do not capture
other threads' calls. Collectors own numeric telemetry only. Poison recovery
preserves available observations without changing inventory results, cancellation,
or native resource ownership.

## Native iterators and memory

Native and compatibility iterators reject 64 consecutive identical successful
values with `BrowseNonProgress`, preserving iterator type, escaped path, repeated
value, consecutive count, and total yielded count. Short duplicate sequences
remain valid.

Native fetched counts are checked against fixed cache capacity before indexing.
String caches are reset before native refills, null entries are skipped, null-only
batches are bounded, and remaining COM-owned strings are freed after failed or
early-ended enumeration. Compatibility wrappers restore the active browse path
when a lower-level non-progress error is root-scoped.

COM-memory wrappers preserve their allocator/ownership pairing. Unsafe comments
state actual pointer validity, initialized element count, ownership, and ABI
requirements. Mock allocations are checked before pointer writes.

## Read, write, and error semantics

Semantic BSTR reads retain empty strings, embedded quotes, and leading/trailing
quotes as data. Native display reads add one presentation pair; other formatting
is unchanged.

Read results preserve requested tag order and length. Item add/read rejections
produce error sentinels and bad-quality details rather than silently removing
rows. Write rejection remains an `Ok(WriteResult)` with `success = false` when
the operation returns a per-tag failure; fatal setup/native errors remain `Err`.
Existing group cleanup attempts and error unwinding are preserved; failed cleanup
is logged.

Quality uses the OPC status bits: `0xC0` is `Good`, `0x00` is `Bad`, `0x40` is
`Uncertain`, and other status patterns return an `Unknown` label.
FILETIME formatting is local time, with `N/A` for zero and `Invalid` for an
unrepresentable time.

Windows COM errors retain `OpcError::Com { source }`, native HRESULT bits, source
chaining, and friendly hints. Contextual errors retain escaped path/item
diagnostics and typed non-progress details. Structured log fields and established
inventory/native-browse/worker/error targets remain stable across module boundaries.

## Acceptance tests

Portable tests cover public paths and models, opaque token parsing, provider
defaults/mocks, control changes, backpressure cleanup, histogram boundaries,
thread-local scopes, entered failures, cancellation, and poisoned telemetry.

Windows mock tests additionally cover protocol mapping, continuation/root
negotiation, deferred DA2 expansion, exact ItemIDs, session limits and cleanup,
foreground connection reuse/retry, read/write results, and connection destruction
on the worker. Windows validation includes the 32-bit target and MSRV.

These tests do not establish live server/DCOM interoperability. The ignored
live-server cursor probe stays excluded from automated acceptance.
