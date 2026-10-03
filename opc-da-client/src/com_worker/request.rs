//! Stable public requests and read presentation intent.

use crate::errors::OpcResult;
use crate::provider::{
    BrowseCapabilities, BrowsePage, BrowsePageRequest, BrowseSessionToken, OpcValue, TagValue,
    WriteResult,
};
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use tokio::sync::oneshot;

/// Controls whether read values preserve machine semantics or use TUI-oriented display formatting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadPresentation {
    /// Return `VT_BSTR` contents exactly as stored by COM.
    Semantic,
    /// Wrap `VT_BSTR` contents in quotes for human-readable display.
    Display,
}

/// Represents an asynchronous request dispatched to the COM worker thread.
pub enum ComRequest {
    /// Request to enumerate OPC DA servers registered on the native Windows machine.
    ListServers {
        /// Provider host label, retained in logs; native enumeration is local.
        host: String,
        /// One-shot channel to send back the server enumeration result.
        reply: oneshot::Sender<OpcResult<Vec<String>>>,
    },
    /// Request to read current values, quality, and timestamps for tag IDs.
    ReadTagValues {
        /// OPC server ProgID.
        server: String,
        /// List of fully qualified tag identifiers to read.
        tag_ids: Vec<String>,
        /// Value formatting intent for this read.
        presentation: ReadPresentation,
        /// One-shot channel to send back the tag values result.
        reply: oneshot::Sender<OpcResult<Vec<TagValue>>>,
    },
    /// Request to write a typed value to a single tag.
    WriteTagValue {
        /// OPC server ProgID.
        server: String,
        /// Tag identifier to write.
        tag_id: String,
        /// Typed value to write.
        value: OpcValue,
        /// One-shot channel to send back the write operation result.
        reply: oneshot::Sender<OpcResult<WriteResult>>,
    },
    /// Request to recursively browse available tags on a server.
    BrowseTags {
        /// OPC server ProgID.
        server: String,
        /// Maximum number of tags to discover before stopping.
        max_tags: usize,
        /// Atomic counter tracking total tags discovered.
        progress: Arc<AtomicUsize>,
        /// Shared mutex-protected vector storing discovered tag names incrementally.
        tags_sink: Arc<std::sync::Mutex<Vec<String>>>,
        /// One-shot channel to send back the complete tag discovery list.
        reply: oneshot::Sender<OpcResult<Vec<String>>>,
    },
    /// Request the native browse capabilities of a server.
    BrowseCapabilities {
        /// OPC server ProgID.
        server: String,
        /// One-shot channel to send back the capabilities.
        reply: oneshot::Sender<OpcResult<BrowseCapabilities>>,
    },
    /// Open an isolated native browse session.
    OpenBrowseSession {
        /// OPC server ProgID.
        server: String,
        /// One-shot channel to send back the opaque session token.
        reply: oneshot::Sender<OpcResult<BrowseSessionToken>>,
    },
    /// Request one bounded native browse page.
    BrowsePage {
        /// Opaque browse session token.
        session: BrowseSessionToken,
        /// One-level browse request.
        request: BrowsePageRequest,
        /// One-shot channel to send back the page.
        reply: oneshot::Sender<OpcResult<BrowsePage>>,
    },
    /// Close an isolated native browse session.
    CloseBrowseSession {
        /// Opaque browse session token.
        session: BrowseSessionToken,
        /// One-shot channel to report completion.
        reply: oneshot::Sender<OpcResult<()>>,
    },
}
