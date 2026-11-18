//! XESP parent check: prints the flags + XESP chain of the
//! enable parents referenced by the Riverwood block actors,
//! to identify the 4th disabled actor (13 -> 9, only 3
//! Stormcloaks explained by XESP parent 0x1b753). The prime
//! suspect is the saddled horse (0xdb3f1, XESP parent
//! 0x93638).
//!
//! Usage: cargo run -p dovah-esm --example xesp_check

use dovah_esm::{Group, Plugin, Record};
use std::collections::HashMap;

fn xesp(record: &Record) -> Option<(u32, u8)> {
    record.field(b"XESP").and_then(|f| {
        if f.data.len() >= 5 {
            Some((
                u32::from_le_bytes(f.data[0..4].try_into().unwrap()),
                f.data[4],
            ))
        } else {
            None
        }
    })
}

fn main() {
    let esm =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test-data/Skyrim.esm");
    let plugin = Plugin::open(&esm).expect("open ESM");
    let mut by_form_id: HashMap<u32, &Record> = HashMap::new();
    for record in plugin.all_records() {
        by_form_id.insert(record.form_id, record);
    }
    let ids: [u32; 7] = [
        0x00093638, // horse's parent
        0x0001b753, // Stormcloak parent (disabled)
        0x0001b754, // Imperial parent (enabled)
        0x000db3f1, // the horse
        0x0001b747, 0x0001b744, 0x0001b743, // Imperials
    ];
    for id in ids {
        match by_form_id.get(&id) {
            Some(r) => {
                let edid = r.edid().unwrap_or_default();
                println!(
                    "0x{id:08x} {} {edid} flags=0x{:08x} (initially-disabled bit 0x800: {}) xesp={:?} has_DATA={}",
                    r.type_str(),
                    r.flags,
                    r.flags & 0x800 != 0,
                    xesp(r),
                    r.field(b"DATA").is_some()
                );
            }
            None => println!("0x{id:08x} NOT FOUND"),
        }
    }
    // Which group files the horse's parent?
    let mut stack: Vec<&Group> = plugin.groups.iter().collect();
    while let Some(g) = stack.pop() {
        for record in g.records() {
            if record.form_id == 0x00093638 {
                println!(
                    "0x00093638 filed in group type={:?} label={:?}",
                    g.group_type,
                    g.label_form_id()
                );
            }
        }
        stack.extend(g.subgroups());
    }
    // Walk the horse's parent chain (ref_enabled semantics:
    // disabled if the parent is initially-disabled or has a
    // disabled ancestor; missing parent = enabled).
    let mut cur = 0x00093638u32;
    for _ in 0..8 {
        let Some(r) = by_form_id.get(&cur) else {
            println!("chain: 0x{cur:08x} missing -> enabled");
            break;
        };
        println!(
            "chain: 0x{cur:08x} {} flags=0x{:08x} disabled={} xesp={:?}",
            r.type_str(),
            r.flags,
            r.flags & 0x800 != 0,
            xesp(r)
        );
        match xesp(r) {
            Some((parent, _)) => cur = parent,
            None => break,
        }
    }
}
