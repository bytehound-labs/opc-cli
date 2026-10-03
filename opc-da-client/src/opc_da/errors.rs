pub(crate) use crate::errors::{
    E_INVALIDARG_HRESULT, E_NOTIMPL_HRESULT, MAX_CONSECUTIVE_EMPTY_DA3_PAGES,
    MAX_CONSECUTIVE_IDENTICAL_BROWSE_VALUES, RPC_X_NULL_REF_POINTER_HRESULT,
    browse_continuation_non_progress_error, browse_non_progress_error, com_hresult,
    contextual_browse_error, is_com_hresult, is_da2_browse_to_fallback_error,
    is_da3_browse_compatibility_error, is_non_progress_browse_error,
};
pub use crate::errors::{
    OpcError, OpcResult, format_hresult, friendly_com_hint, friendly_hresult_hint, log_opc_error,
};
