use opc_da_client::diagnostics::{
    NativeInventoryCanaryConfig, NativeReadCanaryConfig, run_native_inventory_canary,
    run_native_read_canary, write_json_lines,
};
use opc_da_client::{
    BrowseNodeFilter, BrowsePageRequest, BrowseSessionToken, OpcDaClient, OpcProvider,
};
use serde_json::json;
use std::time::Duration;

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    match parse_args(std::env::args().skip(1))? {
        CanaryCommand::Read(config) => {
            let report = run_native_read_canary(config).await?;
            write_json_lines(&report, std::io::stdout())?;
        }
        CanaryCommand::Inventory(config) => {
            let output = std::io::LineWriter::new(std::io::stdout());
            run_native_inventory_canary(config, output).await?;
        }
        CanaryCommand::Browse(config) => run_browse_canary(config).await?,
    }
    Ok(())
}

enum CanaryCommand {
    Read(NativeReadCanaryConfig),
    Inventory(NativeInventoryCanaryConfig),
    Browse(BrowseCanaryConfig),
}

struct BrowseCanaryConfig {
    prog_id: String,
    path: Vec<String>,
    page_size: u32,
    max_pages: u32,
}

fn parse_args(mut args: impl Iterator<Item = String>) -> anyhow::Result<CanaryCommand> {
    let first = args.next().ok_or_else(|| {
        anyhow::anyhow!(
            "usage: native_read_canary <PROGID> <ITEMID> [ITEMID ...] [OPTIONS]\n\
             or: native_read_canary inventory <PROGID> [OPTIONS]\n\
             or: native_read_canary browse <PROGID> [--path <NAME> ...] [OPTIONS]"
        )
    })?;
    match first.as_str() {
        "inventory" => parse_inventory_args(args).map(CanaryCommand::Inventory),
        "browse" => parse_browse_args(args).map(CanaryCommand::Browse),
        _ => parse_read_args(first, args).map(CanaryCommand::Read),
    }
}

fn parse_browse_args(mut args: impl Iterator<Item = String>) -> anyhow::Result<BrowseCanaryConfig> {
    let usage = "usage: native_read_canary browse <PROGID> \
                 [--path <NAME> ...] [--page-size <N>] [--max-pages <N>]";
    let prog_id = args.next().ok_or_else(|| anyhow::anyhow!(usage))?;
    if prog_id.starts_with('-') {
        anyhow::bail!("PROGID cannot be an option; {usage}");
    }

    let mut path = Vec::new();
    let mut page_size = 250;
    let mut max_pages = 100;
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--path" => {
                let value = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--path requires a value; {usage}"))?;
                if value.starts_with('-') {
                    anyhow::bail!("--path requires a branch name; {usage}");
                }
                path.push(value);
            }
            "--page-size" => {
                page_size = parse_u32_value(&mut args, "--page-size", usage)?;
            }
            "--max-pages" => {
                max_pages = parse_u32_value(&mut args, "--max-pages", usage)?;
            }
            _ if argument.starts_with('-') => {
                anyhow::bail!("unknown option {argument:?}; {usage}");
            }
            _ => {
                anyhow::bail!("unexpected argument {argument:?}; use --path for branch names");
            }
        }
    }
    if !(1..=1_000).contains(&page_size) {
        anyhow::bail!("--page-size must be between 1 and 1000");
    }
    if !(1..=1_000).contains(&max_pages) {
        anyhow::bail!("--max-pages must be between 1 and 1000");
    }

    Ok(BrowseCanaryConfig {
        prog_id,
        path,
        page_size,
        max_pages,
    })
}

async fn run_browse_canary(config: BrowseCanaryConfig) -> anyhow::Result<()> {
    let client = OpcDaClient::default();
    println!(
        "{}",
        serde_json::to_string(&json!({
            "event": "browse_start",
            "prog_id": config.prog_id,
            "path": config.path,
            "page_size": config.page_size,
            "max_pages": config.max_pages,
        }))?
    );

    let capabilities = client.browse_capabilities(&config.prog_id).await?;
    println!(
        "{}",
        serde_json::to_string(&json!({
            "event": "browse_capabilities",
            "namespace": format!("{:?}", capabilities.namespace),
            "supports_da2": capabilities.supports_da2,
            "supports_da3": capabilities.supports_da3,
            "max_page_size": capabilities.max_page_size,
        }))?
    );
    let session = client.open_browse_session(&config.prog_id).await?;
    let result = browse_target_and_children(&client, &session, &config).await;
    let close_result = client.close_browse_session(&session).await;
    println!(
        "{}",
        serde_json::to_string(&json!({
            "event": "browse_close",
            "success": close_result.is_ok(),
            "error": close_result.as_ref().err().map(ToString::to_string),
        }))?
    );
    if let Err(error) = close_result
        && result.is_ok()
    {
        return Err(error.into());
    }
    result
}

async fn browse_target_and_children(
    client: &OpcDaClient,
    session: &BrowseSessionToken,
    config: &BrowseCanaryConfig,
) -> anyhow::Result<()> {
    let mut parent = None;
    let mut pages_used = 0_u32;
    for (depth, segment) in config.path.iter().enumerate() {
        let mut continuation = None;
        let node = loop {
            pages_used = pages_used
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("browse page counter overflowed"))?;
            if pages_used > config.max_pages {
                anyhow::bail!(
                    "browse exceeded --max-pages {} before finding path segment {segment:?}",
                    config.max_pages
                );
            }
            let page = client
                .browse_page(
                    session,
                    BrowsePageRequest {
                        parent,
                        filter: BrowseNodeFilter::All,
                        max_elements: config.page_size,
                        continuation,
                    },
                )
                .await?;
            emit_browse_page(depth, segment, parent, &page)?;
            if let Some(node) = page.nodes.iter().find(|node| node.name == *segment) {
                break node.clone();
            }
            continuation = page.continuation;
            if continuation.is_none() {
                anyhow::bail!(
                    "browse path segment {segment:?} was not returned by the server under depth {depth}"
                );
            }
        };
        if !node.kind.has_children() && depth + 1 < config.path.len() {
            anyhow::bail!("browse path segment {segment:?} is not a branch");
        }
        parent = Some(node.token);
        println!(
            "{}",
            serde_json::to_string(&json!({
                "event": "browse_path_selected",
                "depth": depth,
                "name": node.name,
                "item_id": node.item_id,
                "kind": format!("{:?}", node.kind),
                "node_token": node.token.to_string(),
            }))?
        );
    }

    let mut continuation = None;
    let child_depth = config.path.len();
    loop {
        pages_used = pages_used
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("browse page counter overflowed"))?;
        if pages_used > config.max_pages {
            anyhow::bail!(
                "browse exceeded --max-pages {} while listing children",
                config.max_pages
            );
        }
        let page = client
            .browse_page(
                session,
                BrowsePageRequest {
                    parent,
                    filter: BrowseNodeFilter::All,
                    max_elements: config.page_size,
                    continuation,
                },
            )
            .await?;
        emit_browse_page(child_depth, "<children>", parent, &page)?;
        continuation = page.continuation;
        if continuation.is_none() {
            break;
        }
    }
    Ok(())
}

fn emit_browse_page(
    depth: usize,
    requested_name: &str,
    parent: Option<opc_da_client::BrowseNodeToken>,
    page: &opc_da_client::BrowsePage,
) -> anyhow::Result<()> {
    let nodes = page
        .nodes
        .iter()
        .map(|node| {
            json!({
                "name": node.name,
                "item_id": node.item_id,
                "kind": format!("{:?}", node.kind),
                "has_children": node.kind.has_children(),
                "is_item": node.kind.is_item(),
                "node_token": node.token.to_string(),
            })
        })
        .collect::<Vec<_>>();
    println!(
        "{}",
        serde_json::to_string(&json!({
            "event": "browse_page",
            "depth": depth,
            "requested_name": requested_name,
            "parent_node_token": parent.map(|token| token.to_string()),
            "nodes": nodes,
            "continuation": page.continuation.map(|token| token.to_string()),
        }))?
    );
    Ok(())
}

fn parse_read_args(
    prog_id: String,
    mut args: impl Iterator<Item = String>,
) -> anyhow::Result<NativeReadCanaryConfig> {
    let usage = "usage: native_read_canary <PROGID> <ITEMID> [ITEMID ...] \
                 [--update-rate-ms <MS>] [--deadline-secs <SECONDS>]";
    if prog_id.starts_with('-') {
        anyhow::bail!("PROGID cannot be an option; {usage}");
    }

    let mut item_ids = Vec::new();
    let mut requested_update_rate_ms = 1_000;
    let mut deadline_secs = 30_u64;
    let mut update_rate_seen = false;
    let mut deadline_seen = false;

    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--update-rate-ms" => {
                if update_rate_seen {
                    anyhow::bail!("duplicate --update-rate-ms option");
                }
                update_rate_seen = true;
                let value = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--update-rate-ms requires a value; {usage}"))?;
                if value.starts_with('-') {
                    anyhow::bail!("--update-rate-ms requires a numeric value; {usage}");
                }
                requested_update_rate_ms = value
                    .parse()
                    .map_err(|_| anyhow::anyhow!("invalid --update-rate-ms value {value:?}"))?;
            }
            "--deadline-secs" => {
                if deadline_seen {
                    anyhow::bail!("duplicate --deadline-secs option");
                }
                deadline_seen = true;
                let value = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--deadline-secs requires a value; {usage}"))?;
                if value.starts_with('-') {
                    anyhow::bail!("--deadline-secs requires a numeric value; {usage}");
                }
                deadline_secs = value
                    .parse()
                    .map_err(|_| anyhow::anyhow!("invalid --deadline-secs value {value:?}"))?;
            }
            _ if argument.starts_with('-') => {
                anyhow::bail!("unknown option {argument:?}; {usage}");
            }
            _ => item_ids.push(argument),
        }
    }

    if item_ids.is_empty() {
        anyhow::bail!("at least one ITEMID is required; {usage}");
    }

    if !(1..=300).contains(&deadline_secs) {
        anyhow::bail!("--deadline-secs must be between 1 and 300");
    }

    let mut config = NativeReadCanaryConfig::new(prog_id, item_ids);
    config.requested_update_rate_ms = requested_update_rate_ms;
    config.deadline = Duration::from_secs(deadline_secs);
    config
        .validate()
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    Ok(config)
}

fn parse_inventory_args(
    mut args: impl Iterator<Item = String>,
) -> anyhow::Result<NativeInventoryCanaryConfig> {
    let usage = "usage: native_read_canary inventory <PROGID> \
                 [--batch-size <N>] [--max-entries <N>] [--min-interval-ms <MS>] \
                 [--item-rate-per-second <N>] [--start-path <COMPONENT>]... \
                 [--deadline-secs <SECONDS>] \
                 [--cancel-after-secs <SECONDS>]";
    let prog_id = args.next().ok_or_else(|| anyhow::anyhow!(usage))?;
    if prog_id.starts_with('-') {
        anyhow::bail!("PROGID cannot be an option; {usage}");
    }

    let mut config = NativeInventoryCanaryConfig::new(prog_id);
    let mut batch_size_seen = false;
    let mut max_entries_seen = false;
    let mut min_interval_seen = false;
    let mut item_rate_seen = false;
    let mut deadline_seen = false;
    let mut cancel_after_seen = false;

    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--batch-size" => {
                reject_duplicate(&mut batch_size_seen, "--batch-size")?;
                config.options.batch_size = parse_u32_value(&mut args, "--batch-size", usage)?;
            }
            "--max-entries" => {
                reject_duplicate(&mut max_entries_seen, "--max-entries")?;
                let value = parse_u64_value(&mut args, "--max-entries", usage)?;
                config.options.max_entries = Some(value);
            }
            "--min-interval-ms" => {
                reject_duplicate(&mut min_interval_seen, "--min-interval-ms")?;
                let value = parse_u64_value(&mut args, "--min-interval-ms", usage)?;
                config.pacing.min_interval = Duration::from_millis(value);
            }
            "--item-rate-per-second" => {
                reject_duplicate(&mut item_rate_seen, "--item-rate-per-second")?;
                let value = parse_u32_value(&mut args, "--item-rate-per-second", usage)?;
                config.pacing.item_rate_per_second = Some(value);
            }
            "--start-path" => {
                let component =
                    next_non_option_value(&mut args, "--start-path", "path component", usage)?;
                config
                    .start_path
                    .get_or_insert_with(Vec::new)
                    .push(component);
            }
            "--deadline-secs" => {
                reject_duplicate(&mut deadline_seen, "--deadline-secs")?;
                let value = parse_u64_value(&mut args, "--deadline-secs", usage)?;
                config.deadline = Duration::from_secs(value);
            }
            "--cancel-after-secs" => {
                reject_duplicate(&mut cancel_after_seen, "--cancel-after-secs")?;
                let value = parse_u64_value(&mut args, "--cancel-after-secs", usage)?;
                config.cancel_after = Some(Duration::from_secs(value));
            }
            _ if argument.starts_with('-') => {
                anyhow::bail!("unknown option {argument:?}; {usage}");
            }
            _ => {
                anyhow::bail!("unexpected argument {argument:?}; {usage}");
            }
        }
    }

    config
        .validate()
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    Ok(config)
}

fn reject_duplicate(seen: &mut bool, option: &str) -> anyhow::Result<()> {
    if *seen {
        anyhow::bail!("duplicate {option} option");
    }
    *seen = true;
    Ok(())
}

fn parse_u32_value(
    args: &mut impl Iterator<Item = String>,
    option: &str,
    usage: &str,
) -> anyhow::Result<u32> {
    let value = next_numeric_value(args, option, usage)?;
    value
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid {option} value {value:?}"))
}

fn parse_u64_value(
    args: &mut impl Iterator<Item = String>,
    option: &str,
    usage: &str,
) -> anyhow::Result<u64> {
    let value = next_numeric_value(args, option, usage)?;
    value
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid {option} value {value:?}"))
}

fn next_numeric_value(
    args: &mut impl Iterator<Item = String>,
    option: &str,
    usage: &str,
) -> anyhow::Result<String> {
    next_non_option_value(args, option, "numeric value", usage)
}

fn next_non_option_value(
    args: &mut impl Iterator<Item = String>,
    option: &str,
    value_kind: &str,
    usage: &str,
) -> anyhow::Result<String> {
    let value = args
        .next()
        .ok_or_else(|| anyhow::anyhow!("{option} requires a value; {usage}"))?;
    if value.starts_with('-') {
        anyhow::bail!("{option} requires a {value_kind}; {usage}");
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::{CanaryCommand, parse_args};
    use std::time::Duration;

    fn parse(
        arguments: &[&str],
    ) -> anyhow::Result<opc_da_client::diagnostics::NativeReadCanaryConfig> {
        match parse_args(arguments.iter().map(|argument| (*argument).to_string()))? {
            CanaryCommand::Read(config) => Ok(config),
            _ => anyhow::bail!("expected read command"),
        }
    }

    fn parse_browse(arguments: &[&str]) -> anyhow::Result<super::BrowseCanaryConfig> {
        match parse_args(arguments.iter().map(|argument| (*argument).to_string()))? {
            CanaryCommand::Browse(config) => Ok(config),
            _ => anyhow::bail!("expected browse command"),
        }
    }

    #[test]
    fn parses_items_and_options_in_either_order() {
        let config = parse(&[
            "Yokogawa.CSHIS_OPC.1",
            "Tag.A",
            "--update-rate-ms",
            "25",
            "Tag.B",
            "--deadline-secs",
            "10",
        ])
        .unwrap();
        assert_eq!(config.item_ids, ["Tag.A", "Tag.B"]);
        assert_eq!(config.requested_update_rate_ms, 25);
        assert_eq!(config.deadline, std::time::Duration::from_secs(10));
    }

    #[test]
    fn rejects_unknown_options_duplicates_missing_values_and_missing_items() {
        for arguments in [
            &["Server", "Tag", "--unknown"][..],
            &[
                "Server",
                "Tag",
                "--update-rate-ms",
                "1",
                "--update-rate-ms",
                "2",
            ][..],
            &["Server", "Tag", "--deadline-secs"][..],
            &["Server"][..],
        ] {
            assert!(
                parse(arguments).is_err(),
                "accepted invalid arguments: {arguments:?}"
            );
        }
    }

    #[test]
    fn rejects_option_like_values_and_invalid_numbers() {
        for arguments in [
            &["Server", "Tag", "--update-rate-ms", "--deadline-secs"][..],
            &["Server", "Tag", "--deadline-secs", "-1"][..],
            &["Server", "Tag", "--update-rate-ms", "not-a-number"][..],
            &["--not-a-server", "Tag"][..],
        ] {
            assert!(
                parse(arguments).is_err(),
                "accepted invalid arguments: {arguments:?}"
            );
        }
    }

    #[test]
    fn parses_browse_path_and_options() {
        let config = parse_browse(&[
            "browse",
            "Yokogawa.CSHIS_OPC.1",
            "--path",
            "SCS0130",
            "--page-size",
            "25",
            "--max-pages",
            "4",
        ])
        .unwrap();
        assert_eq!(config.prog_id, "Yokogawa.CSHIS_OPC.1");
        assert_eq!(config.path, ["SCS0130"]);
        assert_eq!(config.page_size, 25);
        assert_eq!(config.max_pages, 4);
    }

    #[test]
    fn rejects_out_of_range_timing_values() {
        for arguments in [
            &["Server", "Tag", "--update-rate-ms", "0"][..],
            &["Server", "Tag", "--update-rate-ms", "60001"][..],
            &["Server", "Tag", "--deadline-secs", "0"][..],
            &["Server", "Tag", "--deadline-secs", "301"][..],
        ] {
            assert!(
                parse(arguments).is_err(),
                "accepted out-of-range timing arguments: {arguments:?}"
            );
        }
    }

    fn parse_inventory(
        arguments: &[&str],
    ) -> anyhow::Result<opc_da_client::diagnostics::NativeInventoryCanaryConfig> {
        match parse_args(arguments.iter().map(|argument| (*argument).to_string()))? {
            CanaryCommand::Inventory(config) => Ok(config),
            CanaryCommand::Read(_) => anyhow::bail!("expected inventory command"),
        }
    }

    #[test]
    fn parses_inventory_options_in_any_order() {
        let config = parse_inventory(&[
            "inventory",
            "Yokogawa.CSHIS_OPC.1",
            "--deadline-secs",
            "120",
            "--batch-size",
            "10",
            "--max-entries",
            "147",
            "--min-interval-ms",
            "50",
            "--item-rate-per-second",
            "25",
            "--start-path",
            "FCS0219",
            "--start-path",
            "203FI02005",
            "--cancel-after-secs",
            "5",
        ])
        .unwrap();
        assert_eq!(config.options.batch_size, 10);
        assert_eq!(config.options.max_entries, Some(147));
        assert_eq!(config.pacing.min_interval, Duration::from_millis(50));
        assert_eq!(config.pacing.item_rate_per_second, Some(25));
        assert_eq!(
            config.start_path,
            Some(vec!["FCS0219".to_string(), "203FI02005".to_string()])
        );
        assert_eq!(config.deadline, Duration::from_secs(120));
        assert_eq!(config.cancel_after, Some(Duration::from_secs(5)));
    }

    #[test]
    fn preserves_read_mode_without_an_inventory_prefix() {
        let config = parse(&["Yokogawa.CSHIS_OPC.1", "FCS0201!204FI00510.PV"]).unwrap();
        assert_eq!(config.item_ids, ["FCS0201!204FI00510.PV"]);
    }

    #[test]
    fn rejects_invalid_inventory_options() {
        for arguments in [
            &["inventory", "Server", "--batch-size", "0"][..],
            &["inventory", "Server", "--batch-size", "1001"][..],
            &["inventory", "Server", "--max-entries", "0"][..],
            &["inventory", "Server", "--item-rate-per-second", "0"][..],
            &["inventory", "Server", "--deadline-secs", "0"][..],
            &["inventory", "Server", "--deadline-secs", "601"][..],
            &["inventory", "Server", "--cancel-after-secs", "60"][..],
            &["inventory", "Server", "--batch-size"][..],
            &["inventory", "Server", "--start-path"][..],
            &["inventory", "Server", "--start-path", "-branch"][..],
            &["inventory", "Server", "--start-path", ""][..],
            &[
                "inventory",
                "Server",
                "--min-interval-ms",
                "1",
                "--min-interval-ms",
                "2",
            ][..],
        ] {
            assert!(
                parse_inventory(arguments).is_err(),
                "accepted invalid inventory arguments: {arguments:?}"
            );
        }
    }
}
