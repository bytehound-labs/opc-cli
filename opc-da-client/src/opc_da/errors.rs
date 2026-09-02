use thiserror::Error;
use windows::core::HRESULT;

/// Result type alias for OPC DA operations.
pub type OpcResult<T> = Result<T, OpcError>;

/// Centralized error enum for the OPC DA client.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum OpcError {
    /// Standard Windows COM/DCOM error.
    ///
    /// This variant wraps a [`windows::core::Error`] and provides a friendly
    /// hint for common OPC-related HRESULT codes.
    #[error("COM error: {source} ({})", friendly_hresult_hint(.source.code()).unwrap_or("No hint available"))]
    Com {
        #[from]
        source: windows::core::Error,
    },

    /// Connection-related errors (e.g., host unreachable, resolution failure).
    #[error("Connection failed: {0}")]
    Connection(String),

    /// Server-specific errors reported via OPC status codes.
    #[error("Server error: {0} (0x{1:08X})")]
    Server(String, u32),

    /// Errors during data type conversion or VARIANT processing.
    #[error("Data conversion failed: {0}")]
    Conversion(String),

    /// Operation attempted in an invalid state (e.g., group already exists).
    #[error("Invalid state: {0}")]
    InvalidState(String),

    /// Feature not implemented or supported by the target OPC server.
    #[error("Not implemented: {0}")]
    NotImplemented(String),

    /// A browse iterator returned the same value without making progress.
    #[error(
        "OPC DA browse iterator {iterator_type} made no progress at browse path {browse_path}: \
         repeated value {repeated_value:?} for {consecutive} consecutive results after \
         {yielded} values"
    )]
    BrowseNonProgress {
        iterator_type: String,
        browse_path: String,
        repeated_value: String,
        consecutive: usize,
        yielded: usize,
    },

    /// A DA 3.0 browse continuation failed to make progress.
    #[error("OPC DA browse continuation made no progress at browse path {browse_path}: {detail}")]
    BrowseContinuationNonProgress { browse_path: String, detail: String },

    /// Catch-all for unexpected internal failures.
    #[error("Internal error: {0}")]
    Internal(String),
}

impl From<anyhow::Error> for OpcError {
    fn from(err: anyhow::Error) -> Self {
        Self::Internal(err.to_string())
    }
}

impl From<tokio::task::JoinError> for OpcError {
    fn from(err: tokio::task::JoinError) -> Self {
        Self::Internal(format!("Async task join failed: {err}"))
    }
}

impl From<std::num::TryFromIntError> for OpcError {
    fn from(err: std::num::TryFromIntError) -> Self {
        OpcError::Conversion(format!("Integer conversion error: {err}"))
    }
}

/// Helper to format HRESULT with friendly hints.
pub fn format_hresult(hr: HRESULT) -> String {
    let hex = format!("0x{:08X}", hr.0 as u32);
    match friendly_hresult_hint(hr) {
        Some(hint) => format!("{hex}: {hint}"),
        None => hex,
    }
}

/// Maps known COM/DCOM error codes to actionable user hints.
pub fn friendly_hresult_hint(hr: HRESULT) -> Option<&'static str> {
    match hr.0 as u32 {
        0x80040112 => Some("Server license does not permit OPC client connections"),
        0x80080005 => Some("Server process failed to start — check if it is installed and running"),
        0x80070005 => {
            Some("Access denied — DCOM launch/activation permissions not configured for this user")
        }
        0x800706BA => {
            Some("RPC server unavailable — the target host may be offline or blocking RPC")
        }
        0x800706F4 => Some("COM marshalling error — try restarting the OPC server"),
        0x80040154 => Some("Server is not registered on this machine"),
        0x80004003 => Some("Invalid pointer (E_POINTER)"),
        0xC0040004 => Some("Server rejected write — the item may be read-only (OPC_E_BADRIGHTS)"),
        0xC0040006 => {
            Some("Data type mismatch — server cannot convert the written value (OPC_E_BADTYPE)")
        }
        0xC0040007 => Some("Item ID not found in server address space (OPC_E_UNKNOWNITEMID)"),
        0xC0040008 => Some("Item ID syntax is invalid for this server (OPC_E_INVALIDITEMID)"),
        _ => None,
    }
}

/// Maps an [`OpcError`] to a friendly COM hint if it is a COM error.
pub fn friendly_com_hint(error: &OpcError) -> Option<&'static str> {
    match error {
        OpcError::Com { source: e } => friendly_hresult_hint(e.code()),
        _ => None,
    }
}

pub(crate) const E_INVALIDARG_HRESULT: u32 = 0x8007_0057;
pub(crate) const E_NOTIMPL_HRESULT: u32 = 0x8000_4001;
pub(crate) const RPC_X_NULL_REF_POINTER_HRESULT: u32 = 0x8007_06F4;
pub(crate) const MAX_CONSECUTIVE_IDENTICAL_BROWSE_VALUES: usize = 64;
pub(crate) const MAX_CONSECUTIVE_EMPTY_DA3_PAGES: usize = MAX_CONSECUTIVE_IDENTICAL_BROWSE_VALUES;

pub(crate) fn com_hresult(error: &OpcError) -> Option<u32> {
    match error {
        OpcError::Com { source } => Some(source.code().0 as u32),
        _ => None,
    }
}

pub(crate) fn is_com_hresult(error: &OpcError, expected: u32) -> bool {
    com_hresult(error) == Some(expected)
}

pub(crate) fn is_da3_browse_compatibility_error(error: &OpcError) -> bool {
    is_com_hresult(error, RPC_X_NULL_REF_POINTER_HRESULT)
        || is_com_hresult(error, E_NOTIMPL_HRESULT)
}

pub(crate) fn is_non_progress_browse_error(error: &OpcError) -> bool {
    matches!(error, OpcError::BrowseNonProgress { .. })
}

pub(crate) fn browse_non_progress_error(
    iterator_type: &str,
    browse_path: &[String],
    repeated_value: &str,
    consecutive: usize,
    yielded: usize,
) -> OpcError {
    OpcError::BrowseNonProgress {
        iterator_type: iterator_type.to_string(),
        browse_path: format_browse_path(browse_path),
        repeated_value: repeated_value.to_string(),
        consecutive,
        yielded,
    }
}

pub(crate) fn browse_continuation_non_progress_error(
    browse_path: &[String],
    detail: impl Into<String>,
) -> OpcError {
    OpcError::BrowseContinuationNonProgress {
        browse_path: format_browse_path(browse_path),
        detail: detail.into(),
    }
}

fn format_browse_path(browse_path: &[String]) -> String {
    if browse_path.is_empty() {
        "<root>".to_string()
    } else {
        browse_path
            .iter()
            .map(|part| format!("{part:?}"))
            .collect::<Vec<_>>()
            .join(" > ")
    }
}

pub(crate) fn contextual_browse_error(
    error: OpcError,
    operation: &str,
    browse_path: &[String],
    item_name: Option<&str>,
) -> OpcError {
    let path = format_browse_path(browse_path);
    let item = item_name
        .map(|name| format!(" item {name:?}"))
        .unwrap_or_default();
    let hresult = com_hresult(&error)
        .map(|value| format!("0x{value:08X}"))
        .unwrap_or_else(|| "N/A".to_string());
    let hint = friendly_com_hint(&error).unwrap_or("none");
    let chain = format!("{error:#}");

    tracing::error!(
        operation = %operation,
        browse_path = %path,
        item_name = %item_name.map_or("<none>", |name| name),
        hresult = %hresult,
        hint = %hint,
        chain = %chain,
        "OPC browse operation failed"
    );

    if let OpcError::BrowseNonProgress {
        iterator_type,
        repeated_value,
        consecutive,
        yielded,
        ..
    } = error
    {
        return OpcError::BrowseNonProgress {
            iterator_type,
            browse_path: path,
            repeated_value,
            consecutive,
            yielded,
        };
    }

    OpcError::Internal(format!(
        "OPC DA {operation} failed at browse path {path}{item}: {error}"
    ))
}

/// Emits a structured `tracing::error!` event with machine-parseable fields.
///
/// Extracts the HRESULT code and friendly hint from an [`OpcError`],
/// and logs them as named fields for aggregation by log analysis tools.
///
/// # Arguments
/// * `error` - The OPC error to log
/// * `operation` - Name of the operation that failed (e.g., "read_tag_values")
pub fn log_opc_error(error: &OpcError, operation: &str) {
    let hresult = com_hresult(error).map(|value| format!("0x{value:08X}"));
    let hint = friendly_com_hint(error);
    let chain = format!("{error:#}");

    tracing::error!(
        operation = %operation,
        hresult = hresult.as_deref().unwrap_or("N/A"),
        hint = hint.unwrap_or("none"),
        chain = %chain,
        "OPC operation failed"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_com_hresult_and_matches_expected_code() {
        let error = OpcError::Com {
            source: windows::core::Error::from_hresult(HRESULT(E_INVALIDARG_HRESULT as i32)),
        };
        assert_eq!(com_hresult(&error), Some(E_INVALIDARG_HRESULT));
        assert!(is_com_hresult(&error, E_INVALIDARG_HRESULT));
        assert!(!is_com_hresult(&error, 0));
    }

    #[test]
    fn non_com_errors_have_no_hresult() {
        let error = OpcError::Internal("test".to_string());
        assert_eq!(com_hresult(&error), None);
        assert!(!is_com_hresult(&error, E_INVALIDARG_HRESULT));
    }

    #[test]
    fn da3_browse_fallback_is_limited_to_compatibility_hresult_values() {
        for hresult in [RPC_X_NULL_REF_POINTER_HRESULT, E_NOTIMPL_HRESULT] {
            let error = OpcError::Com {
                source: windows::core::Error::from_hresult(HRESULT(hresult as i32)),
            };
            assert!(is_da3_browse_compatibility_error(&error));
        }

        for hresult in [E_INVALIDARG_HRESULT, 0x8007_0005, 0x8007_06BA] {
            let error = OpcError::Com {
                source: windows::core::Error::from_hresult(HRESULT(hresult as i32)),
            };
            assert!(!is_da3_browse_compatibility_error(&error));
        }
        assert!(!is_da3_browse_compatibility_error(&OpcError::Internal(
            "not a COM compatibility failure".to_string()
        )));
    }

    #[test]
    fn contextual_browse_error_includes_escaped_path_and_item() {
        let error = contextual_browse_error(
            OpcError::Internal("synthetic".to_string()),
            "GetItemID",
            &[String::from("FCS0528"), "\u{1}".to_string()],
            Some("\u{1}"),
        );
        assert!(matches!(
            error,
            OpcError::Internal(message)
                if message.contains("\"FCS0528\" > \"\\u{1}\"")
                    && message.contains("item \"\\u{1}\"")
                    && message.contains("synthetic")
        ));
    }

    #[test]
    fn contextual_browse_error_preserves_non_progress_details_and_updates_path() {
        let error = contextual_browse_error(
            OpcError::BrowseNonProgress {
                iterator_type: "IEnumString".to_string(),
                browse_path: "<root>".to_string(),
                repeated_value: "\u{1}".to_string(),
                consecutive: 64,
                yielded: 147,
            },
            "browse_recursive(leaves)",
            &["SCS0130".to_string(), "FCS0528".to_string()],
            None,
        );

        assert!(matches!(
            error,
            OpcError::BrowseNonProgress {
                iterator_type,
                browse_path,
                repeated_value,
                consecutive,
                yielded,
            } if iterator_type == "IEnumString"
                && browse_path == "\"SCS0130\" > \"FCS0528\""
                && repeated_value == "\u{1}"
                && consecutive == 64
                && yielded == 147
        ));
    }

    #[test]
    fn browse_continuation_non_progress_error_formats_its_path_and_detail() {
        let error = browse_continuation_non_progress_error(
            &["FCS0528".to_string(), "PV".to_string()],
            "server repeated continuation token",
        );
        assert!(matches!(
            &error,
            OpcError::BrowseContinuationNonProgress { browse_path, detail }
                if browse_path == "\"FCS0528\" > \"PV\""
                    && detail == "server repeated continuation token"
        ));
        assert_eq!(
            error.to_string(),
            "OPC DA browse continuation made no progress at browse path \"FCS0528\" > \"PV\": \
             server repeated continuation token"
        );
    }
}
