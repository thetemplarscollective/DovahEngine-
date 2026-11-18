//! UI milestone 2 probe 4: city-marker positions — Tamriel frame
//! or local? Find Solitude's city marker. Dump city WRLD records.

use dovah_esm::{Group, GroupChild, GroupType, Plugin, Record};
use dovah_strings::{StringsFile, StringsKind};

const ESM_PATH: &str = "test-data/Skyrim.esm";
const STRINGS_DIR: &str = "test-data/strings/Data/Strings";

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

fn pos(r: &Record) -> Option<(f32, f32, f32)> {
    let f = r.field(b"DATA")?;
    if f.data.len() < 12 {
        return None;
    }
    Some((
        f32::from_le_bytes(f.data[0..4].try_into().unwrap()),
        f32::from_le_bytes(f.data[4..8].try_into().unwrap()),
        f32::from_le_bytes(f.data[8..12].try_into().unwrap()),
    ))
}

fn main() {
    let general = StringsFile::from_bytes(
        &std::fs::read(format!("{STRINGS_DIR}/Skyrim_English.STRINGS")).unwrap(),
        StringsKind::General,
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
    let by_edid: std::collections::HashMap<String, &Record> = records
        .iter()
        .filter_map(|r| r.edid().map(|e| (e, *r)))
        .collect();

    println!("--- city marker positions ---");
    for edid in [
        "WhiterunMapMarkerREF",
        "RiftenMapMarkerREF",
        "WindhelmMapMarkerRef",
        "MarkarthMapMarkerREF",
        "WhiterunDragonsReachMapMarker",
        "SolitudeBluePalaceMapMarker",
        "WindhelmPalaceOfTheKingsMapMarker",
        "MarkarthPalaceMapMarker",
    ] {
        match by_edid.get(edid) {
            Some(r) => println!("  {edid}: pos={:?}", pos(r)),
            None => println!("  {edid}: NOT FOUND"),
        }
    }
    println!("  (reference: Riverwood marker is at (21941, -44792))");

    println!("--- XMRK refs in Solitude world (0x00037edf) ---");
    // Walk groups; print XMRK REFRs under WorldChildren(0x37edf).
    fn walk_sol<'a>(group: &'a Group, in_sol: bool, name: &dyn Fn(u32) -> String) {
        let mut in_sol = in_sol;
        if group.group_type == GroupType::WorldChildren && group.label_form_id() == Some(0x00037edf)
        {
            in_sol = true;
        }
        for child in &group.children {
            match child {
                GroupChild::Record(r) => {
                    if in_sol && r.type_str() == "REFR" && r.field(b"XMRK").is_some() {
                        let full = r
                            .field(b"FULL")
                            .map(|f| u32::from_le_bytes(f.data[0..4].try_into().unwrap()));
                        let nm = full.map(name).unwrap_or_default();
                        let tnam = r
                            .field(b"TNAM")
                            .map(|f| u16::from_le_bytes(f.data[0..2].try_into().unwrap()));
                        println!(
                            "  {} name='{nm}' tnam={tnam:?} pos={:?}",
                            r.edid().unwrap_or_default(),
                            pos(r)
                        );
                    }
                }
                GroupChild::Group(g) => walk_sol(g, in_sol, name),
            }
        }
    }
    for group in &plugin.groups {
        walk_sol(group, false, &name);
    }

    println!("--- city WRLD records (fields) ---");
    for (label, id) in [
        ("WhiterunWorld", 0x0001a26fu32),
        ("SolitudeWorld", 0x00037edf),
    ] {
        let r = records.iter().find(|r| r.form_id == id).unwrap();
        println!("  {label} 0x{id:08x}:");
        for f in &r.fields {
            let preview = if f.data.len() <= 24 {
                format!("{:02x?}", f.data)
            } else {
                format!("{:02x?}...", &f.data[..24])
            };
            println!("    {} len={} {}", f.type_str(), f.data.len(), preview);
        }
    }
}
