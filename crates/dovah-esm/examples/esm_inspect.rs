//! Inspect a real plugin file: header info, top groups, and a full
//! record sweep.
//!
//! Usage: `cargo run --example esm_inspect -- <path-to-.esm-or-.esp>`
//!
//! Prints the TES4 HEDR (version, record count, next object ID),
//! masters, top-group types, the total parsed record count, how
//! many records were compressed, and the ten most common record
//! types. Exits non-zero if the file fails to parse.

use dovah_esm::Plugin;
use std::collections::HashMap;
use std::env;
use std::process::ExitCode;
use std::time::Instant;

fn main() -> ExitCode {
    let path = match env::args().nth(1) {
        Some(p) => p,
        None => {
            eprintln!("usage: esm_inspect <path-to-.esm-or-.esp>");
            return ExitCode::from(2);
        }
    };
    let started = Instant::now();
    let plugin = match Plugin::open(&path) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("failed to parse {path}: {e}");
            return ExitCode::from(1);
        }
    };
    let parse_secs = started.elapsed().as_secs_f64();
    println!("plugin: {path}");
    println!("parse time: {parse_secs:.2}s");
    match plugin.header_info() {
        Some(info) => {
            println!("HEDR version: {}", info.version);
            println!("HEDR record/group count: {}", info.record_count);
            println!("HEDR next object ID: 0x{:06x}", info.next_object_id);
        }
        None => println!("HEDR: missing or malformed"),
    }
    let masters = plugin.masters();
    println!("masters ({}): {:?}", masters.len(), masters);
    println!("top groups: {}", plugin.groups.len());
    for group in &plugin.groups {
        let label = group
            .label_type()
            .unwrap_or_else(|| "<non-top>".to_string());
        println!("  group {:?} label={label}", group.group_type);
    }
    let records = plugin.all_records();
    println!("total records parsed: {}", records.len());
    fn count_groups(group: &dovah_esm::Group) -> usize {
        1 + group.subgroups().map(count_groups).sum::<usize>()
    }
    let total_groups: usize = plugin.groups.iter().map(count_groups).sum();
    println!("total groups parsed: {total_groups}");
    println!("records + groups: {}", records.len() + total_groups);
    let compressed = records.iter().filter(|r| r.was_compressed()).count();
    println!("compressed records: {compressed}");
    let mut by_type: HashMap<String, usize> = HashMap::new();
    for record in &records {
        *by_type.entry(record.type_str()).or_insert(0) += 1;
    }
    let mut counts: Vec<(String, usize)> = by_type.into_iter().collect();
    counts.sort_by_key(|&(_, count)| std::cmp::Reverse(count));
    println!("top 10 record types:");
    for (kind, count) in counts.iter().take(10) {
        println!("  {kind}: {count}");
    }
    println!("distinct record types: {}", counts.len());
    ExitCode::SUCCESS
}
