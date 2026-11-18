//! Find a cell by editor ID and dump what is inside it.
//!
//! Usage: `cargo run --example esm_cell -- <path-to-.esm> <edid-substring>`
//!
//! Prints each matching CELL record (EDID, form ID, display name,
//! interior flag), then every object placed in it: reference form ID,
//! base record type + editor ID, and the base record's model path
//! when it has one. This is the data the renderer needs to draw a
//! room: cell -> placed references -> base objects -> .nif paths.

use dovah_esm::{Group, GroupType, Plugin, Record};
use std::collections::HashMap;
use std::env;
use std::process::ExitCode;

fn form_id_from_field(record: &Record, field_type: &[u8; 4]) -> Option<u32> {
    let field = record.field(field_type)?;
    if field.data.len() != 4 {
        return None;
    }
    Some(u32::from_le_bytes(field.data[0..4].try_into().unwrap()))
}

/// Recursively collect every group whose label is the given parent
/// form ID and whose type marks it as a cell's children.
fn collect_cell_children<'a>(group: &'a Group, form_id: u32, out: &mut Vec<&'a Group>) {
    if group.label_form_id() == Some(form_id)
        && matches!(
            group.group_type,
            GroupType::CellChildren
                | GroupType::CellPersistentChildren
                | GroupType::CellTemporaryChildren
        )
    {
        out.push(group);
    }
    for subgroup in group.subgroups() {
        collect_cell_children(subgroup, form_id, out);
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: esm_cell <path-to-.esm> <edid-substring>");
        return ExitCode::from(2);
    }
    let needle = args[2].to_lowercase();
    let plugin = match Plugin::open(&args[1]) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("failed to parse {}: {e}", args[1]);
            return ExitCode::from(1);
        }
    };
    // Index every record by form ID for base-object lookups.
    let mut by_form_id: HashMap<u32, &Record> = HashMap::new();
    for record in plugin.all_records() {
        by_form_id.insert(record.form_id, record);
    }
    let mut matches = 0usize;
    for record in plugin.all_records() {
        if record.type_str() != "CELL" {
            continue;
        }
        let edid = match record.edid() {
            Some(e) => e,
            None => continue,
        };
        if !edid.to_lowercase().contains(&needle) {
            continue;
        }
        matches += 1;
        let full = record
            .field(b"FULL")
            .map(|f| f.as_str())
            .unwrap_or_else(|| "<no name>".to_string());
        let interior = record
            .field(b"DATA")
            .and_then(|f| f.data.first())
            .map(|b| b & 0x01 != 0)
            .unwrap_or(false);
        println!(
            "CELL {edid} form=0x{:08x} name=\"{full}\" interior={interior}",
            record.form_id
        );
        let mut child_groups = Vec::new();
        for group in &plugin.groups {
            collect_cell_children(group, record.form_id, &mut child_groups);
        }
        let mut placed: Vec<&Record> = Vec::new();
        for group in child_groups {
            placed.extend(group.records());
        }
        let mut by_type: HashMap<String, usize> = HashMap::new();
        for r in &placed {
            *by_type.entry(r.type_str()).or_insert(0) += 1;
        }
        println!("  placed objects: {} {:?}", placed.len(), by_type);
        // Who is in the room (ACHR base records are NPCs).
        for r in &placed {
            if r.type_str() != "ACHR" {
                continue;
            }
            let base = form_id_from_field(r, b"NAME").and_then(|id| by_form_id.get(&id));
            let who = match base {
                Some(b) => format!("{} {}", b.type_str(), b.edid().unwrap_or_default()),
                None => "<base not in file>".to_string(),
            };
            println!("  ACHR 0x{:08x} -> {who}", r.form_id);
        }
        // How many distinct models the room needs.
        let mut models: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut no_model = 0usize;
        for r in &placed {
            if r.type_str() != "REFR" {
                continue;
            }
            match form_id_from_field(r, b"NAME")
                .and_then(|id| by_form_id.get(&id))
                .and_then(|b| b.field(b"MODL"))
            {
                Some(f) => {
                    models.insert(f.as_str());
                }
                None => no_model += 1,
            }
        }
        println!(
            "  distinct model paths: {} ({} REFRs with no model: triggers/markers)",
            models.len(),
            no_model
        );
        let mut models_sorted: Vec<&String> = models.iter().collect();
        models_sorted.sort();
        for m in models_sorted {
            println!("  MODEL {m}");
        }
        let mut shown = 0usize;
        for r in &placed {
            if r.type_str() != "REFR" {
                continue;
            }
            let base_id = form_id_from_field(r, b"NAME");
            let (base_type, base_edid, model) = match base_id.and_then(|id| by_form_id.get(&id)) {
                Some(base) => (
                    base.type_str(),
                    base.edid().unwrap_or_else(|| "<no edid>".to_string()),
                    base.field(b"MODL").map(|f| f.as_str()),
                ),
                None => ("?".to_string(), "<base not in file>".to_string(), None),
            };
            println!(
                "  REFR 0x{:08x} -> {base_type} {base_edid}{}",
                r.form_id,
                model.map(|m| format!(" [{m}]")).unwrap_or_default()
            );
            shown += 1;
            if shown >= 40 {
                println!("  ... and {} more REFRs", placed.len() - shown);
                break;
            }
        }
    }
    if matches == 0 {
        println!("no CELL with EDID containing '{}'", args[2]);
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}
