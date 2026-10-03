use super::*;

#[test]
fn cancellation_and_deferred_branch_errors_preserve_their_data() {
    assert!(matches!(
        contextual_inventory_error(InventoryError::Cancelled, "browse", &[], None),
        InventoryError::Cancelled
    ));
    let error = InventoryError::InvalidDa2Branch {
        parent_path: vec!["literal.parent!path".to_string()],
        branch: "literal.branch/name".to_string(),
    };
    assert!(matches!(
        contextual_inventory_error(error, "browse", &[], None),
        InventoryError::InvalidDa2Branch { parent_path, branch }
            if parent_path == vec!["literal.parent!path".to_string()]
                && branch == "literal.branch/name"
    ));
}

#[test]
fn failed_inventory_keeps_escaped_browse_context() {
    let error = contextual_inventory_error(
        OpcError::Internal("synthetic failure".to_string()).into(),
        "GetItemID",
        &["literal.parent!path".to_string()],
        Some("literal.item/name"),
    );
    assert!(matches!(
        error,
        InventoryError::Failed(OpcError::Internal(message))
            if message.contains("\"literal.parent!path\"")
                && message.contains("item \"literal.item/name\"")
                && message.contains("synthetic failure")
    ));
}
