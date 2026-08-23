use crate::opc_da::errors::{OpcError, OpcResult};
use async_trait::async_trait;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::sync::mpsc;
use uuid::Uuid;

#[cfg(feature = "test-support")]
use mockall::automock;

/// A single tag's read result.
///
/// Returned by [`OpcProvider::read_tag_values`].
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
    /// The current value as a display string.
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
    Completed(InventoryCompleted),
}

#[derive(Debug)]
struct InventoryControlState {
    cancelled: AtomicBool,
    paused: AtomicBool,
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
            }),
        }
    }

    /// Request cancellation at the next bounded COM boundary.
    pub fn cancel(&self) {
        self.state.cancelled.store(true, Ordering::Release);
    }

    /// Pause before the next bounded COM operation.
    pub fn pause(&self) {
        self.state.paused.store(true, Ordering::Release);
    }

    /// Resume a paused inventory.
    pub fn resume(&self) {
        self.state.paused.store(false, Ordering::Release);
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
        self.control.cancel();
    }

    /// Pause this inventory before its next bounded operation.
    pub fn pause(&self) {
        self.control.pause();
    }

    /// Resume this inventory.
    pub fn resume(&self) {
        self.control.resume();
    }
}

impl Drop for InventoryStream {
    fn drop(&mut self) {
        // Close the receiver before joining so a worker blocked on a full
        // event channel can observe the disconnect and finish.
        self.receiver.close();
        self.control.cancel();
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
    /// # Errors
    /// Returns `Err` if the server connection fails, no items can be added
    /// to the OPC group, or the synchronous read operation fails.
    async fn read_tag_values(&self, server: &str, tag_ids: Vec<String>)
    -> OpcResult<Vec<TagValue>>;

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
