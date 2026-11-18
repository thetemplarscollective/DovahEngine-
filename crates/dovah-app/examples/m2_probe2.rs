//! UI milestone 2 probe 2: close the remaining data questions.
//! Run from the workspace root.
//!
//! 1. What are the SPIT @32 form IDs (0x000c44bf / 0x000f2ca8 /
//!    0x000f2ca8)? SPIT type @8 for a Power and an Ability.
//! 2. RACE DATA skill-boost format on a second race (Breton).
//! 3. AVIF DESC string IDs resolve in .DLSTRINGS?
//! 4. Map markers IN TAMRIEL ONLY: count, FNAM/TNAM values,
//!    what XLRT points at, named markers by type.

use dovah_esm::{Group, GroupChild, GroupType, Plugin, Record};
use dovah_strings::{StringsFile, StringsKind};
use std::collections::{HashMap, HashSet};

const ESM_PATH: &str = "test-data/Skyrim.esm";
const STRINGS_DIR: &str = "test-data/strings/Data/Strings";
const TAMRIEL_WRLD: u32 = 0x0000_003c;

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

fn spit(record: &Record) -> Option<[u32; 9]> {
    let f = record.field(b"SPIT")?;
    if f.data.len() != 36 {
        return None;
    }
    let mut out = [0u32; 9];
    for i in 0..9 {
        out[i] = u32::from_le_bytes(f.data[i * 4..i * 4 + 4].try_into().unwrap());
    }
    Some(out)
}

fn main() {
    let general = StringsFile::from_bytes(
        &std::fs::read(format!("{STRINGS_DIR}/Skyrim_English.STRINGS")).unwrap(),
        StringsKind::General,
    )
    .unwrap();
    let dialog = StringsFile::from_bytes(
        &std::fs::read(format!("{STRINGS_DIR}/Skyrim_English.DLSTRINGS")).unwrap(),
        StringsKind::LengthPrefixed,
    )
    .unwrap();
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
    let by_edid: HashMap<String, &Record> = records
        .iter()
        .filter_map(|r| r.edid().map(|e| (e, *r)))
        .collect();

    println!("--- SPIT @32 form IDs ---");
    for id in [0x000c44bfu32, 0x000f2ca8, 0x000f2caa] {
        match by_id.get(&id) {
            Some(r) => println!(
                "  0x{:08x}: {} ({})",
                id,
                r.type_str(),
                r.edid().unwrap_or_default()
            ),
            None => println!("  0x{id:08x}: NOT IN SKYRIM.ESM"),
        }
    }

    println!("--- SPIT type @8 for Power / Ability ---");
    for (label, id) in [
        ("Battle Cry", 0x000e40c3u32),
        ("Resist Frost", 0x000aa020),
        ("Combat Heal Rate", 0x001031d3),
        ("Flames", 0x00012fcd),
    ] {
        let r = by_id.get(&id).unwrap();
        let s = spit(r).unwrap();
        println!(
            "  {label} 0x{id:08x}: cost={} flags={} type={} charge={:.2} casttype={} range={} perk=0x{:08x} f28={} f32=0x{:08x}",
            s[0],
            s[1],
            s[2],
            f32::from_bits(s[3]),
            s[4],
            s[5],
            s[6],
            s[7],
            s[8]
        );
    }

    println!("--- BretonRace DATA skill boosts ---");
    let breton = by_edid.get("BretonRace").unwrap();
    let data = breton.field(b"DATA").unwrap();
    print!("  first 16 bytes: {:02x?} ->", &data.data[..16]);
    for i in 0..7 {
        let skill = data.data[i * 2] as i8;
        let boost = data.data[i * 2 + 1] as i8;
        if skill >= 0 {
            print!(" (skill {skill}, +{boost})");
        }
    }
    println!();

    println!("--- AVIF DESC in .DLSTRINGS ---");
    for id in [4897u32, 4985, 4921, 13083, 4986, 15062] {
        println!("  DESC {id}: {}", dialog.get(id).unwrap_or("<MISSING>"));
    }

    println!("--- XLRT target ---");
    for id in [0x0010f63cu32] {
        match by_id.get(&id) {
            Some(r) => println!(
                "  0x{id:08x}: {} ({})",
                r.type_str(),
                r.edid().unwrap_or_default()
            ),
            None => println!("  0x{id:08x}: NOT IN SKYRIM.ESM"),
        }
    }

    println!("--- Tamriel map markers ---");
    // Walk the Tamriel WorldChildren group; collect REFRs with XMRK.
    let mut markers: Vec<(&Record, u32)> = Vec::new(); // (record, parent cell)
    fn walk_tamriel<'a>(group: &'a Group, cell: u32, markers: &mut Vec<(&'a Record, u32)>) {
        let mut cell = cell;
        match group.group_type {
            GroupType::CellChildren
            | GroupType::CellPersistentChildren
            | GroupType::CellTemporaryChildren => {
                if let Some(id) = group.label_form_id() {
                    cell = id;
                }
            }
            _ => {}
        }
        for child in &group.children {
            match child {
                GroupChild::Record(r) => {
                    if r.type_str() == "REFR" && r.field(b"XMRK").is_some() {
                        markers.push((r, cell));
                    }
                }
                GroupChild::Group(g) => walk_tamriel(g, cell, markers),
            }
        }
    }
    for group in &plugin.groups {
        if group.group_type != GroupType::Top || group.label_type().as_deref() != Some("WRLD") {
            continue;
        }
        for sub in group.subgroups() {
            if sub.group_type == GroupType::WorldChildren
                && sub.label_form_id() == Some(TAMRIEL_WRLD)
            {
                walk_tamriel(sub, 0, &mut markers);
            }
        }
    }
    println!("  Tamriel XMRK markers: {}", markers.len());
    let mut fnam_hist: HashMap<u8, usize> = HashMap::new();
    let mut tnam_hist: HashMap<u16, usize> = HashMap::new();
    let mut xlrt_set: HashSet<u32> = HashSet::new();
    let mut no_full = 0usize;
    for (r, _cell) in &markers {
        if let Some(f) = r.field(b"FNAM") {
            if !f.data.is_empty() {
                *fnam_hist.entry(f.data[0]).or_insert(0) += 1;
            }
        }
        if let Some(f) = r.field(b"TNAM") {
            if f.data.len() == 2 {
                let v = u16::from_le_bytes(f.data[0..2].try_into().unwrap());
                *tnam_hist.entry(v).or_insert(0) += 1;
            }
        }
        if let Some(f) = r.field(b"XLRT") {
            if f.data.len() == 4 {
                xlrt_set.insert(u32::from_le_bytes(f.data[0..4].try_into().unwrap()));
            }
        }
        if r.field(b"FULL").is_none() {
            no_full += 1;
        }
    }
    println!("  FNAM histogram: {fnam_hist:?}");
    println!("  TNAM histogram: {tnam_hist:?}");
    println!("  distinct XLRT values: {}", xlrt_set.len());
    println!("  markers without FULL: {no_full}");

    let wanted = [
        "Riverwood",
        "Bleak Falls Barrow",
        "Embershard Mine",
        "Guardian Stones",
        "Whiterun",
        "Helgen",
        "Riften",
        "Solitude",
        "Markarth",
        "Windhelm",
        "Falkreath",
        "Morthal",
        "Dawnstar",
        "Winterhold",
        "Dragon Bridge",
        "Ivarstead",
        "Rorikstead",
        "Karthwasten",
    ];
    for (r, cell) in &markers {
        let full_id = r
            .field(b"FULL")
            .map(|f| u32::from_le_bytes(f.data[0..4].try_into().unwrap()));
        let nm = full_id.map(&name).unwrap_or_default();
        if !wanted.contains(&nm.as_str()) {
            continue;
        }
        let fnam = r
            .field(b"FNAM")
            .map(|f| format!("{:02x?}", f.data))
            .unwrap_or_default();
        let tnam = r
            .field(b"TNAM")
            .map(|f| format!("{:02x?}", f.data))
            .unwrap_or_default();
        let xlrt = r
            .field(b"XLRT")
            .map(|f| u32::from_le_bytes(f.data[0..4].try_into().unwrap()))
            .unwrap_or(0);
        let pos = r.field(b"DATA").map(|f| {
            (
                f32::from_le_bytes(f.data[0..4].try_into().unwrap()),
                f32::from_le_bytes(f.data[4..8].try_into().unwrap()),
                f32::from_le_bytes(f.data[8..12].try_into().unwrap()),
            )
        });
        println!(
            "  {nm} ref=0x{:08x} cell=0x{:08x} FNAM={fnam} TNAM={tnam} XLRT=0x{xlrt:08x} pos={pos:?}",
            r.form_id, cell
        );
    }
    // Sample a few markers of each TNAM value for the type mapping.
    let mut shown: HashMap<u16, usize> = HashMap::new();
    for (r, _cell) in &markers {
        let t = r
            .field(b"TNAM")
            .filter(|f| f.data.len() == 2)
            .map(|f| u16::from_le_bytes(f.data[0..2].try_into().unwrap()))
            .unwrap_or(0xffff);
        let n = shown.entry(t).or_insert(0);
        if *n >= 3 {
            continue;
        }
        *n += 1;
        let full_id = r
            .field(b"FULL")
            .map(|f| u32::from_le_bytes(f.data[0..4].try_into().unwrap()));
        let nm = full_id.map(&name).unwrap_or_default();
        println!("  TNAM={t}: {nm}");
    }
}
