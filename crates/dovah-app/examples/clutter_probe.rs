//! M17 clutter-coverage probe (Dylan's ask): is EVERY loose
//! item in the inn pickable? Checks:
//! 1. Which pickupable refs cell.rs drops (the 199 probe refs vs
//!    198 built interactables — find the missing one and why).
//! 2. FLOR refs (25): what they are, and how the harvest
//!    ingredient is specified on the base.
//! 3. WEAP refs (7): where they are, and whether rack weapons
//!    are separate refs or baked into ACTI/STAT models.
//! 4. STAT refs whose EDID looks like loose clutter (table
//!    settings baked into scenery — not pickupable even in the
//!    real game, but the gap must be visible).

use dovah_esm::{Group, GroupType, Plugin, Record};
use std::collections::HashMap;

const FLAG_INITIALLY_DISABLED: u32 = 0x0000_0800;
const FLAG_IS_MARKER: u32 = 0x0080_0000;

const PICKUPABLE_TYPES: &[&str] = &[
    "MISC", "WEAP", "ARMO", "ALCH", "BOOK", "INGR", "AMMO", "KEYM", "SLGM", "NOTE", "SCRL",
];

fn cell_children(plugin: &Plugin, cell_id: u32) -> Vec<&Record> {
    let mut out = Vec::new();
    let mut stack: Vec<&Group> = plugin.groups.iter().collect();
    while let Some(g) = stack.pop() {
        if matches!(
            g.group_type,
            GroupType::CellChildren
                | GroupType::CellPersistentChildren
                | GroupType::CellTemporaryChildren
        ) && g.label_form_id() == Some(cell_id)
        {
            out.extend(g.records());
        }
        stack.extend(g.subgroups());
    }
    out
}

fn base_id(record: &Record) -> Option<u32> {
    record.field(b"NAME").and_then(|f| {
        if f.data.len() == 4 {
            Some(u32::from_le_bytes(f.data[0..4].try_into().unwrap()))
        } else {
            None
        }
    })
}

fn main() {
    let esm = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("test-data")
        .join("Skyrim.esm");
    let plugin = Plugin::open(&esm).expect("open ESM");
    let mut by_form_id: HashMap<u32, &Record> = HashMap::new();
    let mut by_edid: HashMap<String, &Record> = HashMap::new();
    for record in plugin.all_records() {
        by_form_id.insert(record.form_id, record);
        if let Some(edid) = record.edid() {
            by_edid.insert(edid, record);
        }
    }
    let inn = by_edid.get("RiverwoodSleepingGiantInn").expect("inn CELL");
    let children = cell_children(&plugin, inn.form_id);
    println!("inn children: {}", children.len());

    // --- 1. Pickupables dropped by the cell loader ---
    println!("\n== pickupable refs dropped by cell.rs filters ==");
    let mut kept = 0;
    let mut dropped = 0;
    for r in &children {
        if r.type_str() != "REFR" {
            continue;
        }
        let Some(bid) = base_id(r) else { continue };
        let Some(base) = by_form_id.get(&bid) else {
            continue;
        };
        if !PICKUPABLE_TYPES.contains(&base.type_str().as_str()) {
            continue;
        }
        let edid = base.edid().unwrap_or_default();
        // Mirror cell.rs append_objects filters, in order.
        let why = if r.flags & FLAG_INITIALLY_DISABLED != 0 {
            Some("initially disabled")
        } else if r.flags & FLAG_IS_MARKER != 0 {
            Some("marker flag")
        } else if r.field(b"DATA").is_none() {
            Some("no DATA placement")
        } else if base.field(b"MODL").is_none() {
            Some("base has no MODL")
        } else if base
            .field(b"MODL")
            .map(|f| {
                f.as_str().to_ascii_lowercase().starts_with("fx\\")
                    || f.as_str().to_ascii_lowercase().starts_with("fx/")
            })
            .unwrap_or(false)
        {
            Some("fx/ model (effect)")
        } else {
            None
        };
        match why {
            Some(reason) => {
                dropped += 1;
                println!(
                    "  DROPPED ref 0x{:08x} {} {} ({reason})",
                    r.form_id,
                    base.type_str(),
                    edid
                );
            }
            None => kept += 1,
        }
    }
    println!("pickupables kept: {kept}, dropped: {dropped}");

    // --- 2. FLOR refs + one base's fields ---
    println!("\n== FLOR refs (harvestable?) ==");
    let mut flor_bases: Vec<u32> = Vec::new();
    for r in &children {
        if r.type_str() != "REFR" {
            continue;
        }
        let Some(bid) = base_id(r) else { continue };
        let Some(base) = by_form_id.get(&bid) else {
            continue;
        };
        if base.type_str() == "FLOR" {
            println!(
                "  ref 0x{:08x} base 0x{bid:08x} {}",
                r.form_id,
                base.edid().unwrap_or_default()
            );
            if !flor_bases.contains(&bid) {
                flor_bases.push(bid);
            }
        }
    }
    for bid in &flor_bases {
        let base = by_form_id[bid];
        println!(
            "  base 0x{bid:08x} {} fields:",
            base.edid().unwrap_or_default()
        );
        for f in &base.fields {
            let code = String::from_utf8_lossy(&f.field_type);
            let preview: String = if f.data.len() <= 24 {
                f.data
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            } else {
                format!("({} bytes)", f.data.len())
            };
            // Resolve 4-byte fields as form IDs where plausible.
            let resolved = if f.data.len() == 4 {
                let id = u32::from_le_bytes(f.data[0..4].try_into().unwrap());
                match by_form_id.get(&id) {
                    Some(rec) => format!(" -> 0x{id:08x} {} {:?}", rec.type_str(), rec.edid()),
                    None => String::new(),
                }
            } else {
                String::new()
            };
            println!("    {code}: {preview}{resolved}");
        }
    }

    // --- 3. WEAP refs + ACTI racks ---
    println!("\n== WEAP refs ==");
    for r in &children {
        if r.type_str() != "REFR" {
            continue;
        }
        let Some(bid) = base_id(r) else { continue };
        let Some(base) = by_form_id.get(&bid) else {
            continue;
        };
        if base.type_str() == "WEAP" {
            let pos = r.field(b"DATA").map(|f| {
                let x = f32::from_le_bytes(f.data[0..4].try_into().unwrap());
                let y = f32::from_le_bytes(f.data[4..8].try_into().unwrap());
                let z = f32::from_le_bytes(f.data[8..12].try_into().unwrap());
                format!("({x:.0},{y:.0},{z:.0})")
            });
            println!(
                "  ref 0x{:08x} {} pos={} disabled={}",
                r.form_id,
                base.edid().unwrap_or_default(),
                pos.unwrap_or_default(),
                r.flags & FLAG_INITIALLY_DISABLED != 0
            );
        }
    }
    println!("\n== ACTI refs (racks, etc.) ==");
    for r in &children {
        if r.type_str() != "REFR" {
            continue;
        }
        let Some(bid) = base_id(r) else { continue };
        let Some(base) = by_form_id.get(&bid) else {
            continue;
        };
        if base.type_str() == "ACTI" {
            let pos = r.field(b"DATA").map(|f| {
                let x = f32::from_le_bytes(f.data[0..4].try_into().unwrap());
                let y = f32::from_le_bytes(f.data[4..8].try_into().unwrap());
                let z = f32::from_le_bytes(f.data[8..12].try_into().unwrap());
                format!("({x:.0},{y:.0},{z:.0})")
            });
            println!(
                "  ref 0x{:08x} {} pos={}",
                r.form_id,
                base.edid().unwrap_or_default(),
                pos.unwrap_or_default()
            );
        }
    }

    // --- 4. STATs that look like loose clutter ---
    println!("\n== STAT refs with clutter-like EDIDs ==");
    let keywords = [
        "plate",
        "bowl",
        "cup",
        "tankard",
        "basket",
        "bottle",
        "food",
        "bread",
        "cheese",
        "candle",
        "pot",
        "pan",
        "kettle",
        "jug",
        "goblet",
        "fork",
        "knife",
        "spoon",
        "meat",
        "pie",
        "sweetroll",
        "apple",
        "wine",
        "ale",
        "mead",
    ];
    let mut found = 0;
    for r in &children {
        if r.type_str() != "REFR" {
            continue;
        }
        let Some(bid) = base_id(r) else { continue };
        let Some(base) = by_form_id.get(&bid) else {
            continue;
        };
        if base.type_str() != "STAT" {
            continue;
        }
        let edid = base.edid().unwrap_or_default().to_ascii_lowercase();
        if keywords.iter().any(|k| edid.contains(k)) {
            found += 1;
            println!(
                "  ref 0x{:08x} {}",
                r.form_id,
                base.edid().unwrap_or_default()
            );
        }
    }
    if found == 0 {
        println!("  (none — no clutter-looking statics in the inn)");
    }
}
