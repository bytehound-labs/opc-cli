use crate::opc_da::errors::{OpcError, OpcResult};
use async_trait::async_trait;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::mpsc;
use uuid::Uuid;

#[cfg(feature = "test-support")]
use mockall::automock;

/// A single tag's read result.
///
/// Returned by [`OpcProvider::read_tag_values`] and
/// [`OpcProvider::read_tag_values_for_display`].
///
/// # Examples
///
/// ```
/// use opc_da_client::TagValue;
///
/// let tv = TagValue {
///     tag_id: "Simulation.Random.1".to_string(),
///     value: "42.5".to_string(),
///     quality: "Good".to_string(),
///     timestamp: "2026-01-01 00:00:00".to_string(),
/// };
/// assert_eq!(tv.tag_id, "Simulation.Random.1");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagValue {
    /// The fully qualified tag identifier (e.g., `"Channel1.Device1.Tag1"`).
    pub tag_id: String,
    /// The current value as a string representation.
    ///
    /// [`OpcProvider::read_tag_values`] preserves `VT_BSTR` contents exactly.
    /// [`OpcProvider::read_tag_values_for_display`] may add presentation quotes
    /// around BSTR values.
    pub value: String,
    /// OPC quality indicator (e.g., `"Good"`, `"Bad"`, or `"Uncertain"`).
    pub quality: String,
    /// Timestamp of the last value change, formatted as a local time string.
    pub timestamp: String,
}

/// Typed value to write to an OPC DA tag.
///
/// # Examples
///
/// ```
/// use opc_da_client::OpcValue;
///
/// let v = OpcValue::Float(3.14);
/// assert_eq!(v, OpcValue::Float(3.14));
/// ```
#[derive(Debug, Clone, PartialEq)]
pub enum OpcValue {
    /// String value (`VT_BSTR`) — server may coerce to target type.
    String(String),
    /// 32-bit integer (`VT_I4`).
    Int(i32),
    /// 64-bit float (`VT_R8`).
    Float(f64),
    /// Boolean (`VT_BOOL`).
    Bool(bool),
}

/// Result of a single write operation.
///
/// # Examples
///
/// ```
/// use opc_da_client::WriteResult;
///
/// let wr = WriteResult {
///     tag_id: "Tag1".to_string(),
///     success: true,
///     error: None,
/// };
/// assert!(wr.success);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteResult {
    /// The tag that was written to.
    pub tag_id: String,
    /// Whether the write succeeded.
    pub success: bool,
    /// Error message if the write failed, `None` on success.
    pub error: Option<String>,
}

/// OPC DA address-space organization reported by a server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowseNamespace {
    /// All item IDs live in one flat namespace.
    Flat,
    /// Items are organized under browsable branches.
    Hierarchical,
    /// The server supports DA 3.0 browsing but does not expose the DA 2.x
    /// namespace query needed to classify its organization.
    Unknown,
}

/// Native browse features available from an OPC DA server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BrowseCapabilities {
    /// Address-space organization reported by the server.
    pub namespace: BrowseNamespace,
    /// Whether native OPC DA 3.0 `IOPCBrowse` is available.
    pub supports_da3: bool,
    /// Whether OPC DA 2.x `IOPCBrowseServerAddressSpace` is available.
    pub supports_da2: bool,
    /// Largest page size accepted by [`OpcProvider::browse_page`].
    pub max_page_size: u32,
}

macro_rules! opaque_browse_token {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Clone, Copy, PartialEq, Eq, Hash)]
        pub struct $name(Uuid);

        impl $name {
            pub(crate) fn new() -> Self {
                Self(Uuid::new_v4())
            }

            /// Parse a token previously encoded with [`ToString::to_string`].
            ///
            /// This allows transport adapters to round-trip opaque tokens
            /// without accessing their internal representation.
            pub fn parse(value: &str) -> Result<Self, uuid::Error> {
                value.parse()
            }
        }

        impl std::fmt::Debug for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter
                    .debug_tuple(stringify!($name))
                    .field(&self.0)
                    .finish()
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.fmt(formatter)
            }
        }

        impl std::str::FromStr for $name {
            type Err = uuid::Error;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                value.parse().map(Self)
            }
        }
    };
}

opaque_browse_token!(
    BrowseSessionToken,
    "Opaque identifier for a browse session owned by the COM worker."
);
opaque_browse_token!(
    BrowseNodeToken,
    "Opaque identifier for a node returned by a browse session."
);
opaque_browse_token!(
    BrowsePageToken,
    "Opaque continuation token for the next bounded browse page."
);

/// Kinds of nodes returned by a native browse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowseNodeKind {
    /// A branch that can have children but is not itself an item.
    Branch,
    /// An item that has no browsable children.
    Item,
    /// A node that is both an item and a branch.
    BranchAndItem,
}

impl BrowseNodeKind {
    /// Returns whether the node can be used as the parent of another browse.
    pub fn has_children(self) -> bool {
        matches!(self, Self::Branch | Self::BranchAndItem)
    }

    /// Returns whether the node identifies a selectable OPC item.
    pub fn is_item(self) -> bool {
        matches!(self, Self::Item | Self::BranchAndItem)
    }
}

/// Node-kind filter for a one-level browse request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowseNodeFilter {
    /// Return only branches.
    Branches,
    /// Return only items.
    Items,
    /// Return both branches and items.
    All,
}

/// One address-space node returned by [`OpcProvider::browse_page`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowseNode {
    /// Opaque token used to browse this node's immediate children.
    pub token: BrowseNodeToken,
    /// Display name relative to the requested parent.
    pub name: String,
    /// Exact fully-qualified item ID when the server supplies one.
    pub item_id: Option<String>,
    /// Whether this node is a branch, item, or both.
    pub kind: BrowseNodeKind,
}

/// Parameters for one bounded, non-recursive browse operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BrowsePageRequest {
    /// Parent node, or `None` to browse the root.
    pub parent: Option<BrowseNodeToken>,
    /// Node kinds to return.
    pub filter: BrowseNodeFilter,
    /// Maximum number of nodes to return.
    pub max_elements: u32,
    /// Opaque continuation returned by the preceding page.
    pub continuation: Option<BrowsePageToken>,
}

/// One bounded page of immediate address-space children.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowsePage {
    /// Nodes returned in this page.
    pub nodes: Vec<BrowseNode>,
    /// Opaque token for the next page, or `None` when enumeration is complete.
    pub continuation: Option<BrowsePageToken>,
}

/// Maximum number of native entries requested for one inventory slice.
pub const MAX_INVENTORY_BATCH_SIZE: u32 = 1_000;

/// Options controlling one bounded namespace inventory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InventoryOptions {
    /// Maximum number of native entries requested per browse operation.
    pub batch_size: u32,
    /// Optional safety cap for a deliberately bounded inventory.
    pub max_entries: Option<u64>,
}

impl Default for InventoryOptions {
    fn default() -> Self {
        Self {
            batch_size: 100,
            max_entries: None,
        }
    }
}

/// One selectable OPC DA item discovered during inventory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryEntry {
    /// Local display name returned by the server.
    pub display_name: String,
    /// Exact ItemID returned by the server.
    pub item_id: String,
    /// Whether the item is also a browsable branch.
    pub kind: BrowseNodeKind,
    /// Stable display labels for the item's ancestors.
    pub breadcrumbs: Vec<String>,
}

/// Progress emitted between bounded inventory operations.
#[derive(Debug, Clone, PartialEq)]
pub struct InventoryProgress {
    pub branches_visited: u64,
    pub entries_seen: u64,
    pub unique_items: u64,
    pub active_time_ms: u64,
    pub paused_time_ms: u64,
    pub items_per_second: f64,
    pub estimated_remaining_ms: Option<u64>,
}

/// Transport used for one bounded inventory slice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InventorySliceBackend {
    /// OPC DA 3.0 `IOPCBrowse`.
    Da3,
    /// OPC DA 2.x `IOPCBrowseServerAddressSpace`.
    Da2,
}

/// Observation emitted after each bounded inventory slice.
///
/// A slice is one page-sized inventory step. Native DA2 enumeration may use
/// several COM calls to produce one slice; `native_operations` reports that
/// bounded work without exposing COM implementation details.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventorySliceObservation {
    /// Monotonically increasing slice number, starting at one.
    pub sequence: u64,
    /// Native browsing protocol used by the slice.
    pub backend: InventorySliceBackend,
    /// Number of nodes returned by the slice.
    pub nodes_returned: u64,
    /// Whether another slice remains for the same branch.
    pub has_more: bool,
    /// Number of bounded native operations performed for this slice.
    pub native_operations: u64,
    /// Wall-clock duration of the slice, including pacing waits.
    pub elapsed_ms: u64,
    /// Total native nodes observed through this slice.
    pub entries_seen: u64,
    /// Total unique item IDs observed through this slice.
    pub unique_items: u64,
}

/// Terminal result for one inventory operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryCompleted {
    pub complete: bool,
    pub cancelled: bool,
    pub truncated: bool,
    pub warning: Option<String>,
    pub capabilities: BrowseCapabilities,
}

/// Event emitted by [`InventoryStream`].
#[derive(Debug, Clone, PartialEq)]
pub enum InventoryEvent {
    Entry(InventoryEntry),
    Progress(InventoryProgress),
    Slice(InventorySliceObservation),
    Completed(InventoryCompleted),
}

#[derive(Debug)]
struct InventoryControlState {
    cancelled: AtomicBool,
    paused: AtomicBool,
    pacing_interval_ns: AtomicU64,
    pacing_item_rate_per_second: AtomicU64,
    batch_size: AtomicUsize,
}

/// Dynamic pacing enforced before bounded native inventory operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InventoryPacing {
    /// Minimum interval between the starts of native operations.
    pub min_interval: Duration,
    /// Maximum number of inventory items requested per second.
    ///
    /// DA3 page operations are charged by their requested page size. Native
    /// DA2 string enumeration is charged by each actual `IEnumString::Next`
    /// refill, using the iterator cache capacity; values already held in that
    /// cache do not consume another pacing budget. Pure test iterators retain
    /// the one-item default cost.
    pub item_rate_per_second: Option<u32>,
}

impl Default for InventoryPacing {
    fn default() -> Self {
        Self {
            min_interval: Duration::ZERO,
            item_rate_per_second: None,
        }
    }
}

/// Control handle for a running inventory.
#[derive(Clone, Debug)]
pub struct InventoryControl {
    state: Arc<InventoryControlState>,
}

impl InventoryControl {
    pub(crate) fn new() -> Self {
        Self {
            state: Arc::new(InventoryControlState {
                cancelled: AtomicBool::new(false),
                paused: AtomicBool::new(false),
                pacing_interval_ns: AtomicU64::new(0),
                pacing_item_rate_per_second: AtomicU64::new(0),
                batch_size: AtomicUsize::new(0),
            }),
        }
    }

    pub(crate) fn new_with_batch_size(batch_size: u32) -> Self {
        debug_assert!((1..=MAX_INVENTORY_BATCH_SIZE).contains(&batch_size));
        let control = Self::new();
        control
            .state
            .batch_size
            .store(batch_size as usize, Ordering::Release);
        control
    }

    /// Request cancellation at the next bounded COM boundary.
    pub fn cancel(&self) {
        self.cancel_with_reason("unspecified");
    }

    /// Request cancellation with a diagnostic source label.
    ///
    /// The label is intentionally diagnostic-only and does not change the
    /// cancellation semantics or the public stream result.
    pub fn cancel_with_reason(&self, reason: &str) {
        let already_cancelled = self.state.cancelled.swap(true, Ordering::Release);
        tracing::warn!(
            cancellation_source = %reason,
            already_cancelled,
            "OPC namespace inventory cancellation requested"
        );
    }

    /// Pause before the next bounded COM operation.
    pub fn pause(&self) {
        self.state.paused.store(true, Ordering::Release);
    }

    /// Resume a paused inventory.
    pub fn resume(&self) {
        self.state.paused.store(false, Ordering::Release);
    }

    /// Replace the pacing applied before the next bounded native operation.
    ///
    /// Updates are observed by a running inventory without restarting it.
    pub fn set_pacing(&self, pacing: InventoryPacing) {
        let nanos = u64::try_from(pacing.min_interval.as_nanos().min(u128::from(u64::MAX)))
            .unwrap_or(u64::MAX);
        self.state
            .pacing_interval_ns
            .store(nanos, Ordering::Release);
        self.state.pacing_item_rate_per_second.store(
            u64::from(pacing.item_rate_per_second.unwrap_or(0)),
            Ordering::Release,
        );
    }

    /// Return the pacing currently applied to this inventory.
    pub fn pacing(&self) -> InventoryPacing {
        InventoryPacing {
            min_interval: Duration::from_nanos(
                self.state.pacing_interval_ns.load(Ordering::Acquire),
            ),
            item_rate_per_second: match self
                .state
                .pacing_item_rate_per_second
                .load(Ordering::Acquire)
            {
                0 => None,
                rate => Some(u32::try_from(rate).unwrap_or(u32::MAX)),
            },
        }
    }

    /// Replace the batch size used for the next inventory slice.
    ///
    /// The value must be between 1 and [`MAX_INVENTORY_BATCH_SIZE`].
    pub fn set_batch_size(&self, batch_size: u32) -> OpcResult<()> {
        if !(1..=MAX_INVENTORY_BATCH_SIZE).contains(&batch_size) {
            return Err(OpcError::InvalidState(format!(
                "Inventory batch size must be between 1 and {MAX_INVENTORY_BATCH_SIZE}"
            )));
        }
        self.state
            .batch_size
            .store(batch_size as usize, Ordering::Release);
        Ok(())
    }

    pub(crate) fn batch_size(&self) -> Option<u32> {
        let batch_size = self.state.batch_size.load(Ordering::Acquire);
        u32::try_from(batch_size).ok().filter(|value| *value != 0)
    }

    /// Return whether cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.state.cancelled.load(Ordering::Acquire)
    }

    pub(crate) fn is_paused(&self) -> bool {
        self.state.paused.load(Ordering::Acquire)
    }
}

/// Cancellable stream of bounded inventory events.
pub struct InventoryStream {
    receiver: mpsc::Receiver<OpcResult<InventoryEvent>>,
    control: InventoryControl,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl InventoryStream {
    pub(crate) fn new(
        receiver: mpsc::Receiver<OpcResult<InventoryEvent>>,
        control: InventoryControl,
        worker: std::thread::JoinHandle<()>,
    ) -> Self {
        Self {
            receiver,
            control,
            worker: Some(worker),
        }
    }

    /// Wait for the next inventory event.
    ///
    /// A failed inventory is delivered as `Some(Err(_))`, which is the
    /// terminal event for the stream. A successful or cancelled inventory
    /// ends with an [`InventoryEvent::Completed`] message.
    pub async fn message(&mut self) -> Option<OpcResult<InventoryEvent>> {
        self.receiver.recv().await
    }

    /// Return a control handle for this inventory.
    pub fn control(&self) -> InventoryControl {
        self.control.clone()
    }

    /// Request cancellation of this inventory.
    pub fn cancel(&self) {
        self.control.cancel_with_reason("stream_cancel");
    }

    /// Pause this inventory before its next bounded operation.
    pub fn pause(&self) {
        self.control.pause();
    }

    /// Resume this inventory.
    pub fn resume(&self) {
        self.control.resume();
    }

    /// Replace the pacing applied before the next bounded native operation.
    pub fn set_pacing(&self, pacing: InventoryPacing) {
        self.control.set_pacing(pacing);
    }

    /// Replace the batch size used for the next inventory slice.
    pub fn set_batch_size(&self, batch_size: u32) -> OpcResult<()> {
        self.control.set_batch_size(batch_size)
    }
}

impl Drop for InventoryStream {
    fn drop(&mut self) {
        // Close the receiver before joining so a worker blocked on a full
        // event channel can observe the disconnect and finish.
        self.receiver.close();
        self.control.cancel_with_reason("stream_drop");
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod inventory_stream_tests {
    use super::*;

    #[test]
    fn dropping_inventory_stream_cancels_and_joins_worker() {
        let control = InventoryControl::new();
        let worker_control = control.clone();
        let finished = Arc::new(AtomicBool::new(false));
        let worker_finished = Arc::clone(&finished);
        let (_sender, receiver) = mpsc::channel(1);
        let worker = std::thread::spawn(move || {
            while !worker_control.is_cancelled() {
                std::thread::yield_now();
            }
            worker_finished.store(true, Ordering::Release);
        });

        drop(InventoryStream::new(receiver, control, worker));
        assert!(finished.load(Ordering::Acquire));
    }
}

#[cfg(test)]
mod read_display_fallback_tests {
    use super::*;

    struct FallbackProvider;

    #[async_trait]
    impl OpcProvider for FallbackProvider {
        async fn list_servers(&self, _host: &str) -> OpcResult<Vec<String>> {
            Ok(Vec::new())
        }

        async fn browse_tags(
            &self,
            _server: &str,
            _max_tags: usize,
            _progress: Arc<AtomicUsize>,
            _tags_sink: Arc<std::sync::Mutex<Vec<String>>>,
        ) -> OpcResult<Vec<String>> {
            Ok(Vec::new())
        }

        async fn read_tag_values(
            &self,
            _server: &str,
            tag_ids: Vec<String>,
        ) -> OpcResult<Vec<TagValue>> {
            Ok(tag_ids
                .into_iter()
                .map(|tag_id| TagValue {
                    tag_id,
                    value: "AUT".to_string(),
                    quality: "Good".to_string(),
                    timestamp: String::new(),
                })
                .collect())
        }

        async fn write_tag_value(
            &self,
            _server: &str,
            tag_id: &str,
            _value: OpcValue,
        ) -> OpcResult<WriteResult> {
            Ok(WriteResult {
                tag_id: tag_id.to_string(),
                success: true,
                error: None,
            })
        }
    }

    #[tokio::test]
    async fn display_read_defaults_to_semantic_read() {
        let values = FallbackProvider
            .read_tag_values_for_display("Server", vec!["Tag".to_string()])
            .await
            .unwrap();

        assert_eq!(values[0].value, "AUT");
    }
}

/// Async trait for OPC DA operations.
///
/// This is the stable public API. Backend implementations provide
/// the actual COM/DCOM interaction.
#[cfg_attr(feature = "test-support", automock)]
#[async_trait]
pub trait OpcProvider: Send + Sync {
    /// List available OPC DA servers on the given host.
    ///
    /// # Errors
    /// Returns `Err` if COM initialization fails or the server registry
    /// cannot be enumerated.
    async fn list_servers(&self, host: &str) -> OpcResult<Vec<String>>;

    /// Browse tags recursively, pushing discoveries to `tags_sink`.
    ///
    /// # Errors
    /// Returns `Err` if the server connection fails, the `ProgID` cannot be
    /// resolved, or the namespace walk encounters an unrecoverable error.
    async fn browse_tags(
        &self,
        server: &str,
        max_tags: usize,
        progress: Arc<AtomicUsize>,
        tags_sink: Arc<std::sync::Mutex<Vec<String>>>,
    ) -> OpcResult<Vec<String>>;

    /// Return the native browse capabilities of an OPC DA server.
    ///
    /// # Errors
    /// Returns `Err` if the server cannot be connected or exposes no supported
    /// OPC DA browse interface.
    async fn browse_capabilities(&self, server: &str) -> OpcResult<BrowseCapabilities> {
        let _ = server;
        Err(OpcError::NotImplemented(
            "Native browsing is not implemented by this provider".to_string(),
        ))
    }

    /// Open an isolated native browse session with its own server connection.
    ///
    /// # Errors
    /// Returns `Err` if the server cannot be connected, browsing is unsupported,
    /// or the worker's bounded session capacity has been reached.
    async fn open_browse_session(&self, server: &str) -> OpcResult<BrowseSessionToken> {
        let _ = server;
        Err(OpcError::NotImplemented(
            "Native browsing is not implemented by this provider".to_string(),
        ))
    }

    /// Browse one bounded level of an open native browse session.
    ///
    /// # Errors
    /// Returns `Err` for invalid, closed, or expired tokens; invalid page sizes;
    /// unsupported requests; or underlying OPC browse failures.
    async fn browse_page(
        &self,
        session: &BrowseSessionToken,
        request: BrowsePageRequest,
    ) -> OpcResult<BrowsePage> {
        let _ = (session, request);
        Err(OpcError::NotImplemented(
            "Native browsing is not implemented by this provider".to_string(),
        ))
    }

    /// Explicitly close a native browse session and release its server connection.
    ///
    /// # Errors
    /// Returns `Err` if the session token is invalid, expired, or already closed.
    async fn close_browse_session(&self, session: &BrowseSessionToken) -> OpcResult<()> {
        let _ = session;
        Err(OpcError::NotImplemented(
            "Native browsing is not implemented by this provider".to_string(),
        ))
    }

    /// Start a cancellable, bounded namespace inventory on a separate
    /// connection from foreground operations.
    ///
    /// The returned stream never exposes interactive browse-session tokens;
    /// all traversal state remains private to the inventory worker.
    async fn start_inventory(
        &self,
        server: &str,
        options: InventoryOptions,
    ) -> OpcResult<InventoryStream> {
        let _ = (server, options);
        Err(OpcError::NotImplemented(
            "Namespace inventory is not implemented by this provider".to_string(),
        ))
    }

    /// Read current values for the given tag IDs.
    ///
    /// `VT_BSTR` values preserve their exact COM contents. No quote characters
    /// are added or removed, so this method is suitable for machine consumers.
    ///
    /// # Errors
    /// Returns `Err` if the server connection fails, no items can be added
    /// to the OPC group, or the synchronous read operation fails.
    async fn read_tag_values(&self, server: &str, tag_ids: Vec<String>)
    -> OpcResult<Vec<TagValue>>;

    /// Read current values formatted for human-readable display.
    ///
    /// The native provider wraps `VT_BSTR` contents in quote characters while
    /// leaving all other value formatting unchanged. The default implementation
    /// delegates to [`Self::read_tag_values`] so third-party providers remain
    /// source-compatible.
    ///
    /// # Errors
    /// Returns the same errors as [`Self::read_tag_values`].
    async fn read_tag_values_for_display(
        &self,
        server: &str,
        tag_ids: Vec<String>,
    ) -> OpcResult<Vec<TagValue>> {
        self.read_tag_values(server, tag_ids).await
    }

    /// Write a value to a single OPC DA tag.
    ///
    /// # Errors
    /// Returns `Err` if the server connection fails, the tag cannot be added
    /// to the OPC group, or the synchronous write operation fails.
    async fn write_tag_value(
        &self,
        server: &str,
        tag_id: &str,
        value: OpcValue,
    ) -> OpcResult<WriteResult>;
}
