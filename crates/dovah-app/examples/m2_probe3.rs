//! UI milestone 2 probe 3: where do the major-city map markers
//! live? Walk ALL groups tracking (world, cell) context and print
//! every REFR whose EDID contains "MapMarker".

use dovah_esm::{Group, GroupChild, GroupType, Plugin, Record};
use dovah_strings::{StringsFile, StringsKind};

const ESM_PATH: &str = "test-data/Skyrim.esm";
const STRINGS_DIR: &str = "test-data/strings/Data/Strings";

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

    fn walk<'a>(
        group: &'a Group,
        world: u32,
        cell: u32,
        name: &dyn Fn(u32) -> String,
        out: &mut Vec<String>,
    ) {
        let mut world = world;
        let mut cell = cell;
        match group.group_type {
            GroupType::WorldChildren => {
                if let Some(id) = group.label_form_id() {
                    world = id;
                }
            }
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
                    let edid = r.edid().unwrap_or_default();
                    if r.type_str() == "REFR" && edid.contains("MapMarker") {
                        let full = r
                            .field(b"FULL")
                            .map(|f| u32::from_le_bytes(f.data[0..4].try_into().unwrap()));
                        let nm = full.map(name).unwrap_or_default();
                        let tnam = r.field(b"TNAM").map(|f| {
                            if f.data.len() == 2 {
                                u16::from_le_bytes(f.data[0..2].try_into().unwrap()) as i32
                            } else {
                                -1
                            }
                        });
                        let xmrk = r.field(b"XMRK").is_some();
                        out.push(format!(
                            "  {edid} name='{nm}' world=0x{world:08x} cell=0x{cell:08x} tnam={tnam:?} xmrk={xmrk}"
                        ));
                    }
                }
                GroupChild::Group(g) => walk(g, world, cell, name, out),
            }
        }
    }

    let mut out = Vec::new();
    for group in &plugin.groups {
        walk(group, 0, 0, &name, &mut out);
    }
    out.sort();
    println!("MapMarker EDID refs: {}", out.len());
    // Print only the city ones + a count by world.
    for line in &out {
        if line.contains("Whiterun")
            || line.contains("Solitude")
            || line.contains("Markarth")
            || line.contains("Riften")
            || line.contains("Windhelm")
            || line.contains("Winterhold")
            || line.contains("College")
        {
            println!("{line}");
        }
    }
    let mut by_world = std::collections::HashMap::new();
    for line in &out {
        let w = line.split("world=0x").nth(1).unwrap()[..8].to_string();
        *by_world.entry(w).or_insert(0usize) += 1;
    }
    println!("by world: {by_world:?}");
    // Also: any MapMarker EDID ref WITHOUT XMRK?
    let no_xmrk = out.iter().filter(|l| l.contains("xmrk=false")).count();
    println!("MapMarker EDIDs without XMRK field: {no_xmrk}");
}
