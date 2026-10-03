#![allow(unsafe_code, unreachable_pub)]
#![cfg_attr(all(windows, feature = "opc-da-backend"), doc = include_str!("../README.md"))]
//! # opc-da-client
//!
//! Backend-agnostic OPC DA client library for Rust — async, trait-based,
//! with transparent COM management.
//!
//! ## Quick Start
//!
//! ```no_run
//! # use anyhow::Result;
//! # #[tokio::main]
//! # async fn main() -> Result<()> {
//! # #[cfg(all(windows, feature = "opc-da-backend"))] {
//! use opc_da_client::{OpcDaClient, OpcProvider};
//!
//! let client = OpcDaClient::default();
//! let servers = client.list_servers("localhost").await?;
//! # }
//! # Ok(())
//! # }
//! ```
//!
//! ## Feature Flags
//!
//! | Flag | Default | Effect |
//! |------|---------|--------|
//! | `opc-da-backend` | ✅ | Native OPC DA backend via `windows-rs` |
//! | `test-support` | ❌ | Enables `MockOpcProvider` via `mockall` |
//!
//! ## Platform
//!
//! The provider trait, value models, opaque tokens, inventory controls, and
//! telemetry models are portable. The native backend and COM worker require
//! Windows because OPC DA is built on COM/DCOM.

#[cfg(windows)]
mod com_guard;
#[cfg(windows)]
pub use com_guard::ComGuard;
mod errors;
#[cfg(all(windows, feature = "opc-da-backend"))]
mod helpers;
#[cfg(all(windows, feature = "opc-da-backend"))]
mod inventory;
#[cfg(any(all(windows, feature = "opc-da-backend"), test))]
#[path = "inventory/boundary.rs"]
mod inventory_boundary;
#[cfg(any(all(windows, feature = "opc-da-backend"), test))]
#[path = "inventory/error.rs"]
mod inventory_error;
#[cfg(any(all(windows, feature = "opc-da-backend"), test))]
#[path = "inventory/telemetry.rs"]
mod inventory_telemetry;
#[cfg(all(windows, feature = "opc-da-backend"))]
mod native_browse;
mod provider;

#[cfg(test)]
mod tests;

#[cfg(all(windows, feature = "opc-da-backend"))]
#[allow(warnings)]
mod bindings;
#[cfg(all(windows, feature = "opc-da-backend"))]
pub mod com_worker;

#[cfg(all(windows, feature = "opc-da-backend"))]
#[allow(warnings)]
mod opc_da;

#[cfg(all(windows, feature = "opc-da-backend"))]
mod backend;

// Stable public API
#[cfg(windows)]
pub use errors::format_hresult;
pub use errors::{OpcError, OpcResult, friendly_com_hint, log_opc_error};
pub use provider::{
    BrowseCapabilities, BrowseNamespace, BrowseNode, BrowseNodeFilter, BrowseNodeKind,
    BrowseNodeToken, BrowsePage, BrowsePageRequest, BrowsePageToken, BrowseSessionToken,
    INVENTORY_NATIVE_OPERATION_LATENCY_BUCKET_UPPER_BOUNDS_NS, InventoryCompleted,
    InventoryControl, InventoryEntry, InventoryEvent, InventoryNativeOperationKind,
    InventoryNativeOperationLatencyHistogram, InventoryNativeOperationObservation,
    InventoryNativeOperationPercentiles, InventoryOptions, InventoryPacing, InventoryProgress,
    InventorySliceBackend, InventorySliceObservation, InventoryStream, MAX_INVENTORY_BATCH_SIZE,
    OpcProvider, OpcValue, TagValue, WriteResult,
};

#[cfg(all(windows, feature = "opc-da-backend"))]
pub use opc_da::typedefs::{GroupHandle, ItemHandle};

// Backend re-exports (conditional)
#[cfg(all(windows, feature = "opc-da-backend"))]
pub use backend::{connector::ComConnector, opc_da::OpcDaClient};

// Test support re-export
#[cfg(feature = "test-support")]
pub use provider::MockOpcProvider;
