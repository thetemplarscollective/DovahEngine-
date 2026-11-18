//! UI milestone 2 data probe: answer the character-menu data
//! questions from Dylan's real Skyrim.esm, not the docs.
//! Run from the workspace root.
//!
//! 1. SPEL: how many, what fields (SPIT layout, EFID list),
//!    and where does a spell's SCHOOL live? (hypothesis: the
//!    first effect's MGEF record carries the skill link)
//! 2. Player NPC_ 0x00000007: which spells (SPLO) + race (RNAM)?
//! 3. Race records: where do the starting skill boosts live?
//!    Which powers (SPLO) does a race grant?
//! 4. AVIF: which records are the 18 skills (EDID/FULL/DESC)?
//! 5. Map markers: REFRs with XMRK — how many, what type
//!    values, does FULL resolve in .STRINGS?
//! 6. LAND in Tamriel: how many cells, what grid extents?
//! 7. SHOU: how many shouts exist (player knows none at start)?

use dovah_esm::{Group, GroupChild, GroupType, Plugin, Record};
use dovah_strings::{StringsFile, StringsKind};
use std::collections::{HashMap, HashSet};

const ESM_PATH: &str = "test-data/Skyrim.esm";
const STRINGS_DIR: &str = "test-data/strings/Data/Strings";
const TAMRIEL_WRLD: u32 = 0x0000_003c;

fn u32_field(record: &Record, code: &[u8; 4]) -> Option<u32> {
    let f = record.field(code)?;
    if f.data.len() != 4 {
        return None;
    }
    Some(u32::from_le_bytes(f.data[0..4].try_into().unwrap()))
}

fn all_records(plugin: &Plugin) -> Vec<&Record> {
    fn walk<'a>(group: &'a Group, out: &mut Vec<&'a Record>) {
        for child in &group.children {
            match child {
                GroupChild::Record(r) => out.push(r),
                GroupChild::Group(g) => walk(g, out),
            }
        }
    }
    let mut out = Vec::new();
    for group in &plugin.groups {
        walk(group, &mut out);
    }
    out
}

fn dump_fields(tag: &str, record: &Record) {
    println!(
        "  {tag} 0x{:08x} ({})",
        record.form_id,
        record.edid().unwrap_or_default()
    );
    for f in &record.fields {
        let preview = if f.data.len() <= 40 {
            format!("{:02x?}", f.data)
        } else {
            format!("{:02x?}...", &f.data[..40])
        };
        println!("    {} len={} {}", f.type_str(), f.data.len(), preview);
    }
}

fn main() {
    let general = StringsFile::from_bytes(
        &std::fs::read(format!("{STRINGS_DIR}/Skyrim_English.STRINGS")).unwrap(),
        StringsKind::General,
    )
    .unwrap();
    println!("strings: {} general", general.len());
    let name = |id: u32| -> String {
        general
            .get(id)
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("<{id}>"))
    };

    let bytes = std::fs::read(ESM_PATH).unwrap();
    let plugin = Plugin::from_bytes(&bytes).unwrap();
    let records = all_records(&plugin);
    let by_id: HashMap<u32, &Record> = records.iter().map(|r| (r.form_id, *r)).collect();
    println!("records: {}", records.len());

    // --- 1. SPEL census + samples ---
    let spels: Vec<&&Record> = records.iter().filter(|r| r.type_str() == "SPEL").collect();
    println!("\n--- SPEL: {} records ---", spels.len());
    let mut spit_sizes: HashMap<usize, usize> = HashMap::new();
    let mut spel_types: HashMap<u32, usize> = HashMap::new();
    for s in &spels {
        if let Some(f) = s.field(b"SPIT") {
            *spit_sizes.entry(f.data.len()).or_insert(0) += 1;
            // SPIT first u32 is the spell type (to verify).
            if f.data.len() >= 4 {
                let t = u32::from_le_bytes(f.data[0..4].try_into().unwrap());
                *spel_types.entry(t).or_insert(0) += 1;
            }
        }
    }
    println!("SPIT sizes: {spit_sizes:?}");
    println!("SPIT first-u32 values: {spel_types:?}");
    for edid in ["Flames", "Healing", "BattleCry", "Firebolt"] {
        if let Some(s) = spels.iter().find(|r| r.edid().as_deref() == Some(edid)) {
            dump_fields("SPEL", s);
            if let Some(full) = u32_field(s, b"FULL") {
                println!("    FULL -> {}", name(full));
            }
            // Resolve the first effect's MGEF.
            for ef in s.fields(b"EFID") {
                if ef.data.len() == 4 {
                    let mgef_id = u32::from_le_bytes(ef.data[0..4].try_into().unwrap());
                    if let Some(m) = by_id.get(&mgef_id) {
                        dump_fields("  MGEF", m);
                        if let Some(full) = u32_field(m, b"FULL") {
                            println!("    MGEF FULL -> {}", name(full));
                        }
                    }
                    break;
                }
            }
        } else {
            println!("  SPEL {edid}: NOT FOUND");
        }
    }

    // --- 2. Player record ---
    println!("\n--- Player NPC_ 0x00000007 ---");
    if let Some(p) = by_id.get(&0x0000_0007) {
        dump_fields("NPC_", p);
        if let Some(race) = u32_field(p, b"RNAM") {
            println!(
                "  race: 0x{race:08x} ({})",
                by_id.get(&race).and_then(|r| r.edid()).unwrap_or_default()
            );
        }
        for splo in p.fields(b"SPLO") {
            if splo.data.len() == 4 {
                let id = u32::from_le_bytes(splo.data[0..4].try_into().unwrap());
                let r = by_id.get(&id);
                let full = r.and_then(|r| u32_field(r, b"FULL")).map(&name);
                println!(
                    "  SPLO 0x{id:08x} {} {}",
                    r.map(|r| r.type_str()).unwrap_or_default(),
                    full.unwrap_or_default()
                );
            }
        }
    }

    // --- 3. NordRace: skill boosts + powers ---
    println!("\n--- NordRace 0x00013746 ---");
    if let Some(n) = by_id.get(&0x0001_3746) {
        dump_fields("RACE", n);
        for splo in n.fields(b"SPLO") {
            if splo.data.len() == 4 {
                let id = u32::from_le_bytes(splo.data[0..4].try_into().unwrap());
                let r = by_id.get(&id);
                let full = r.and_then(|r| u32_field(r, b"FULL")).map(&name);
                println!(
                    "  SPLO 0x{id:08x} {} {}",
                    r.map(|r| r.type_str()).unwrap_or_default(),
                    full.unwrap_or_default()
                );
            }
        }
    }

    // --- 4. AVIF skills ---
    println!("\n--- AVIF (actor values) ---");
    let avifs: Vec<&&Record> = records.iter().filter(|r| r.type_str() == "AVIF").collect();
    println!("AVIF records: {}", avifs.len());
    for a in &avifs {
        let edid = a.edid().unwrap_or_default();
        let full = u32_field(a, b"FULL").map(&name).unwrap_or_default();
        let desc = a
            .field(b"DESC")
            .and_then(|f| {
                if f.data.len() == 4 {
                    Some(u32::from_le_bytes(f.data[0..4].try_into().unwrap()))
                } else {
                    None
                }
            })
            .map(&name);
        println!(
            "  0x{:08x} {:<22} FULL={full} DESC={}",
            a.form_id,
            edid,
            desc.unwrap_or_default()
        );
    }

    // --- 5. XMRK map markers ---
    println!("\n--- XMRK map markers ---");
    let mut xmrk_count = 0usize;
    let mut type_hist: HashMap<u8, usize> = HashMap::new();
    let mut full_ok = 0usize;
    let mut full_miss = 0usize;
    let mut samples: Vec<(u32, u8, String, [f32; 3])> = Vec::new();
    for r in &records {
        if r.type_str() != "REFR" {
            continue;
        }
        let Some(x) = r.field(b"XMRK") else { continue };
        xmrk_count += 1;
        // XMRK layout (to verify): flags u8? type u8? — dump first bytes.
        if x.data.len() >= 2 {
            *type_hist.entry(x.data[1]).or_insert(0) += 1;
        }
        let full = u32_field(r, b"FULL").map(&name);
        match &full {
            Some(f) if !f.starts_with('<') => full_ok += 1,
            _ => full_miss += 1,
        }
        if samples.len() < 8 {
            let pos = r.field(b"DATA").and_then(|f| {
                if f.data.len() >= 12 {
                    Some([
                        f32::from_le_bytes(f.data[0..4].try_into().unwrap()),
                        f32::from_le_bytes(f.data[4..8].try_into().unwrap()),
                        f32::from_le_bytes(f.data[8..12].try_into().unwrap()),
                    ])
                } else {
                    None
                }
            });
            samples.push((
                r.form_id,
                x.data.get(1).copied().unwrap_or(0),
                full.unwrap_or_default(),
                pos.unwrap_or([0.0, 0.0, 0.0]),
            ));
        }
        if xmrk_count == 1 {
            println!(
                "  first XMRK (ref 0x{:08x}): len={} bytes {:02x?}",
                r.form_id,
                x.data.len(),
                x.data
            );
            dump_fields("REFR", r);
        }
    }
    println!("REFRs with XMRK: {xmrk_count}");
    println!("type byte (offset 1) histogram: {type_hist:?}");
    println!("FULL resolves: {full_ok}, misses: {full_miss}");
    for (id, t, n, p) in samples {
        println!(
            "  ref 0x{id:08x} type={t} name={n} pos=({:.0},{:.0},{:.0})",
            p[0], p[1], p[2]
        );
    }

    // --- 6. LAND in Tamriel ---
    println!("\n--- LAND cells in Tamriel ---");
    let mut stack: Vec<&Group> = plugin.groups.iter().collect();
    let mut tamriel_group: Option<&Group> = None;
    while let Some(g) = stack.pop() {
        if g.group_type == GroupType::WorldChildren && g.label_form_id() == Some(TAMRIEL_WRLD) {
            tamriel_group = Some(g);
            break;
        }
        stack.extend(g.subgroups());
    }
    let mut land_grids: HashSet<(i32, i32)> = HashSet::new();
    let mut cell_count = 0usize;
    if let Some(tg) = tamriel_group {
        // Walk the Tamriel group tracking the current CELL's XCLC.
        fn walk(
            g: &Group,
            current: &mut Option<(i32, i32)>,
            land: &mut HashSet<(i32, i32)>,
            cells: &mut usize,
        ) {
            for child in &g.children {
                match child {
                    GroupChild::Record(r) => {
                        if r.type_str() == "CELL" {
                            *cells += 1;
                            if let Some(f) = r.field(b"XCLC") {
                                if f.data.len() >= 8 {
                                    let x = i32::from_le_bytes(f.data[0..4].try_into().unwrap());
                                    let y = i32::from_le_bytes(f.data[4..8].try_into().unwrap());
                                    *current = Some((x, y));
                                }
                            }
                        } else if r.type_str() == "LAND" {
                            if let Some(g) = current {
                                land.insert(*g);
                            }
                        }
                    }
                    GroupChild::Group(sub) => {
                        // CELL groups: the CELL record is the first
                        // child, LAND is in a child group — the
                        // current grid carries down.
                        let saved = *current;
                        walk(sub, current, land, cells);
                        *current = saved;
                    }
                }
            }
        }
        let mut current = None;
        walk(tg, &mut current, &mut land_grids, &mut cell_count);
    }
    let (mut min_x, mut max_x, mut min_y, mut max_y) = (i32::MAX, i32::MIN, i32::MAX, i32::MIN);
    for (x, y) in &land_grids {
        min_x = min_x.min(*x);
        max_x = max_x.max(*x);
        min_y = min_y.min(*y);
        max_y = max_y.max(*y);
    }
    println!("CELL records in Tamriel group: {cell_count}");
    println!("LAND cells: {}", land_grids.len());
    println!(
        "grid extents: x {min_x}..{max_x} ({} wide), y {min_y}..{max_y} ({} tall)",
        max_x - min_x + 1,
        max_y - min_y + 1
    );

    // --- 7. SHOU census ---
    let shous: Vec<&&Record> = records.iter().filter(|r| r.type_str() == "SHOU").collect();
    println!("\n--- SHOU: {} records ---", shous.len());
    for s in shous.iter().take(3) {
        dump_fields("SHOU", s);
    }
}
