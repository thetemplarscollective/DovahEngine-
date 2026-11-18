//! One-off M17 probe: what is OTFT 0x0001697c (the player
//! record's default outfit), and what do its INAM entries point
//! to? The catalog build resolved 0 starting-outfit pieces from
//! it — this finds out why.

use std::collections::HashMap;

fn main() {
    let esm = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("test-data")
        .join("Skyrim.esm");
    let plugin = dovah_esm::Plugin::open(&esm).expect("open ESM");
    let mut by_form_id: HashMap<u32, &dovah_esm::Record> = HashMap::new();
    for record in plugin.all_records() {
        by_form_id.insert(record.form_id, record);
    }
    println!("records indexed: {}", by_form_id.len());
    let Some(outfit) = by_form_id.get(&0x0001_697c) else {
        println!("OTFT 0x0001697c NOT FOUND in by_form_id");
        return;
    };
    println!(
        "0x0001697c: type={} edid={:?} field_count={}",
        outfit.type_str(),
        outfit.edid(),
        outfit.fields.len()
    );
    for f in &outfit.fields {
        let code = String::from_utf8_lossy(&f.field_type);
        println!("  field {code} len={}", f.data.len());
    }
    for inam in outfit.fields(b"INAM") {
        println!("  INAM len={} bytes", inam.data.len());
        for chunk in inam.data.as_chunks::<4>().0 {
            let id = u32::from_le_bytes(*chunk);
            match by_form_id.get(&id) {
                Some(rec) => println!(
                    "    -> 0x{id:08x} type={} edid={:?}",
                    rec.type_str(),
                    rec.edid()
                ),
                None => println!("    -> 0x{id:08x} NOT FOUND"),
            }
        }
    }
    // Also: does the player record's DOFT resolve the same way
    // the catalog does it?
    let player = by_form_id.get(&0x0000_0007).expect("player record");
    let doft = player.field(b"DOFT").expect("DOFT");
    let outfit_id = u32::from_le_bytes(doft.data[0..4].try_into().unwrap());
    println!("player DOFT -> 0x{outfit_id:08x}");

    // The outfit's entry is a leveled list — dump it.
    let list = by_form_id.get(&0x0001_3739).expect("ArmorIronSetAll");
    println!(
        "LVLI 0x00013739: type={} edid={:?}",
        list.type_str(),
        list.edid()
    );
    if let Some(lvld) = list.field(b"LVLD") {
        println!("  LVLD: {:02x?}", lvld.data);
    }
    for f in list.fields(b"LVLO") {
        let level = i16::from_le_bytes(f.data[0..2].try_into().unwrap());
        let item = u32::from_le_bytes(f.data[4..8].try_into().unwrap());
        let count = i16::from_le_bytes(f.data[8..10].try_into().unwrap());
        let desc = match by_form_id.get(&item) {
            Some(rec) => format!("type={} edid={:?}", rec.type_str(), rec.edid()),
            None => "NOT FOUND".to_string(),
        };
        println!("  LVLO level={level} count={count} item=0x{item:08x} {desc}");
        // One level of nesting: if the entry is itself a list,
        // show its entries too.
        if let Some(inner) = by_form_id.get(&item) {
            if inner.type_str() == "LVLI" {
                for g in inner.fields(b"LVLO") {
                    let l2 = i16::from_le_bytes(g.data[0..2].try_into().unwrap());
                    let i2 = u32::from_le_bytes(g.data[4..8].try_into().unwrap());
                    let c2 = i16::from_le_bytes(g.data[8..10].try_into().unwrap());
                    let d2 = match by_form_id.get(&i2) {
                        Some(rec) => {
                            format!("type={} edid={:?}", rec.type_str(), rec.edid())
                        }
                        None => "NOT FOUND".to_string(),
                    };
                    println!("    LVLO level={l2} count={c2} item=0x{i2:08x} {d2}");
                }
            }
        }
    }
}
