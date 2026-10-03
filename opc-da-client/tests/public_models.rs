use opc_da_client::{
    BrowseNode, BrowseNodeKind, BrowseNodeToken, BrowsePageToken, BrowseSessionToken,
    InventoryNativeOperationLatencyHistogram, InventoryOptions, OpcError, OpcValue, TagValue,
    WriteResult, friendly_com_hint,
};

#[test]
fn public_tokens_and_nodes_preserve_opaque_identity() {
    let encoded = "550e8400-e29b-41d4-a716-446655440000";
    assert_eq!(
        BrowseSessionToken::parse(encoded).unwrap().to_string(),
        encoded
    );
    assert_eq!(
        BrowsePageToken::parse(encoded).unwrap().to_string(),
        encoded
    );
    let node = BrowseNode {
        token: BrowseNodeToken::parse(encoded).unwrap(),
        name: "literal.branch!item/PV".to_string(),
        item_id: Some("literal.branch!item/PV".to_string()),
        kind: BrowseNodeKind::BranchAndItem,
    };
    assert_eq!(node.item_id.as_deref(), Some("literal.branch!item/PV"));
    assert!(node.kind.is_item());
    assert!(node.kind.has_children());
}

#[test]
fn public_value_and_write_models_preserve_machine_strings() {
    let value = TagValue {
        tag_id: "raw!AUT".to_string(),
        value: "\"AUT\"".to_string(),
        quality: "Good".to_string(),
        timestamp: "opaque".to_string(),
    };
    assert_eq!(value.value, "\"AUT\"");
    assert_eq!(
        OpcValue::String(value.value),
        OpcValue::String("\"AUT\"".to_string())
    );
    let rejected = WriteResult {
        tag_id: value.tag_id,
        success: false,
        error: Some("read-only".to_string()),
    };
    assert!(!rejected.success);
    assert_eq!(rejected.error.as_deref(), Some("read-only"));
}

#[test]
fn public_inventory_defaults_and_histograms_are_platform_independent() {
    assert_eq!(InventoryOptions::default().batch_size, 100);
    assert_eq!(InventoryOptions::default().max_entries, None);
    let mut histogram = InventoryNativeOperationLatencyHistogram::default();
    histogram.record(std::time::Duration::from_micros(50));
    assert_eq!(histogram.bucket_counts[0], 1);
    assert_eq!(
        histogram
            .percentiles(1, std::time::Duration::from_micros(50))
            .p99,
        std::time::Duration::from_micros(50)
    );
}

#[test]
fn public_errors_keep_conversion_and_display_semantics() {
    let error = OpcError::from(u8::try_from(256_u16).unwrap_err());
    assert!(matches!(&error, OpcError::Conversion(_)));
    assert!(
        error
            .to_string()
            .starts_with("Data conversion failed: Integer conversion error:")
    );
    assert_eq!(friendly_com_hint(&error), None);
    assert!(std::error::Error::source(&error).is_none());
}

#[cfg(feature = "test-support")]
#[tokio::test]
async fn mock_provider_uses_the_same_portable_public_trait() {
    use opc_da_client::{MockOpcProvider, OpcProvider};
    let mut provider = MockOpcProvider::new();
    provider
        .expect_list_servers()
        .withf(|host| host == "test-host")
        .returning(|_| Ok(vec!["Mock.Server".to_string()]));
    assert_eq!(
        provider.list_servers("test-host").await.unwrap(),
        vec!["Mock.Server".to_string()]
    );
}

#[cfg(all(windows, feature = "opc-da-backend"))]
#[test]
fn public_worker_paths_remain_source_compatible() {
    use opc_da_client::ComConnector;
    use opc_da_client::com_worker::{ComRequest, ComWorker, ReadPresentation};
    let worker = ComWorker::<ComConnector>::closed();
    assert!(worker.sender.is_closed());
    let (reply, _receiver) = tokio::sync::oneshot::channel();
    let request = ComRequest::ReadTagValues {
        server: "Mock.Server".to_string(),
        tag_ids: vec!["raw!PV".to_string()],
        presentation: ReadPresentation::Semantic,
        reply,
    };
    assert!(matches!(
        request,
        ComRequest::ReadTagValues {
            presentation: ReadPresentation::Semantic,
            ..
        }
    ));
}
