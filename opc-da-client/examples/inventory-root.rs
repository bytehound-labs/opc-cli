#[cfg(windows)]
use anyhow::{Context, Result, bail};
#[cfg(windows)]
use opc_da_client::{
    InventoryEvent, InventoryOptions, InventoryProgress, OpcDaClient, OpcProvider,
};
#[cfg(windows)]
use std::env;
#[cfg(windows)]
use std::fs::File;
#[cfg(windows)]
use std::io::{BufWriter, Write};
#[cfg(windows)]
use std::time::Instant;

#[cfg(windows)]
fn usage() -> ! {
    eprintln!(
        "usage: inventory-root <server> <root-item-id> <output-jsonl> [batch-size] [max-entries]"
    );
    std::process::exit(2);
}

#[cfg(windows)]
fn parse_u32(value: Option<&String>, name: &str, default: u32) -> Result<u32> {
    value
        .map(|value| {
            value
                .parse()
                .with_context(|| format!("invalid {name}: {value}"))
        })
        .transpose()
        .map(|value| value.unwrap_or(default))
}

#[cfg(windows)]
fn parse_u64(value: Option<&String>, name: &str, default: u64) -> Result<u64> {
    value
        .map(|value| {
            value
                .parse()
                .with_context(|| format!("invalid {name}: {value}"))
        })
        .transpose()
        .map(|value| value.unwrap_or(default))
}

#[cfg(windows)]
fn write_progress(output: &mut BufWriter<File>, progress: &InventoryProgress) -> Result<()> {
    writeln!(
        output,
        "{{\"type\":\"progress\",\"entries_seen\":{},\"unique_items\":{},\"active_time_ms\":{},\"paused_time_ms\":{}}}",
        progress.entries_seen,
        progress.unique_items,
        progress.active_time_ms,
        progress.paused_time_ms
    )?;
    output.flush()?;
    Ok(())
}

#[cfg(windows)]
#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    if args.len() < 4 || args.len() > 6 {
        usage();
    }
    let server = &args[1];
    let root_item_id = &args[2];
    let output_path = &args[3];
    let batch_size = parse_u32(args.get(4), "batch size", 256)?;
    let max_entries = parse_u64(args.get(5), "max entries", 10_000)?;
    if batch_size == 0 || max_entries == 0 {
        bail!("batch size and max entries must be greater than zero");
    }

    let file =
        File::create(output_path).with_context(|| format!("creating output file {output_path}"))?;
    let mut output = BufWriter::new(file);
    let client = OpcDaClient::default();
    let started = Instant::now();
    let mut stream = client
        .start_inventory_at_root(
            server,
            root_item_id,
            InventoryOptions {
                batch_size,
                max_entries: Some(max_entries),
            },
        )
        .await
        .context("starting inventory")?;

    let mut entries = 0_u64;
    while let Some(event) = stream.message().await {
        match event.context("inventory stream error")? {
            InventoryEvent::Entry(entry) => {
                entries += 1;
                writeln!(
                    output,
                    "{{\"type\":\"entry\",\"item_id\":{},\"display_name\":{},\"kind\":{},\"breadcrumbs\":{}}}",
                    serde_json::to_string(&entry.item_id)?,
                    serde_json::to_string(&entry.display_name)?,
                    serde_json::to_string(&format!("{:?}", entry.kind))?,
                    serde_json::to_string(&entry.breadcrumbs)?
                )?;
            }
            InventoryEvent::Progress(value) => {
                write_progress(&mut output, &value)?;
            }
            InventoryEvent::Slice(_) => {}
            InventoryEvent::Completed(completed) => {
                writeln!(
                    output,
                    "{{\"type\":\"completed\",\"complete\":{},\"cancelled\":{},\"truncated\":{},\"warning\":{},\"entries\":{},\"elapsed_ms\":{}}}",
                    completed.complete,
                    completed.cancelled,
                    completed.truncated,
                    serde_json::to_string(&completed.warning)?,
                    entries,
                    started.elapsed().as_millis()
                )?;
                output.flush()?;
                if !completed.complete && !completed.truncated {
                    bail!("inventory did not complete: {:?}", completed.warning);
                }
                return Ok(());
            }
        }
    }

    bail!("inventory stream ended without a completion event")
}

#[cfg(not(windows))]
fn main() {
    eprintln!("inventory-root is only supported on Windows");
}
