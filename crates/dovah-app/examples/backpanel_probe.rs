//! Secret-door probe: what is the back panel (0x00093bdb)?
//! Dumps the ref's fields (enable parent? linked ref? script?)
//! and the panel NIF's shape bounds, to decide whether it should
//! disappear when the secret door opens.
//!
//! Usage: cargo run -p dovah-app --example backpanel_probe

use dovah_bsa::BsaArchive;
use dovah_esm::Plugin;
use dovah_nif::NifFile;
use std::collections::HashMap;
use std::fs::{self, File};

const ESM_PATH: &str = "test-data/Skyrim.esm";
const PANEL_REF: u32 = 0x00093bdb;
const DOOR_REF: u32 = 0x00093bcd;

fn normalize(path: &str) -> String {
    path.replace('\\', "/").to_lowercase()
}

fn main() {
    let plugin = Plugin::open(ESM_PATH).expect("open Skyrim.esm");
    for form_id in [PANEL_REF, DOOR_REF] {
        let Some(r) = plugin
            .all_records()
            .into_iter()
            .find(|r| r.form_id == form_id)
        else {
            println!("ref 0x{form_id:08x} NOT FOUND");
            continue;
        };
        println!(
            "\nref 0x{:08x} type {} flags 0x{:08x}",
            r.form_id,
            r.type_str(),
            r.flags
        );
        for f in &r.fields {
            let ty = String::from_utf8_lossy(&f.field_type).to_string();
            let data = if f.data.len() <= 32 {
                format!("{:02x?}", f.data)
            } else {
                format!("{} bytes", f.data.len())
            };
            println!("  field {ty} ({}): {data}", f.data.len());
            if ty == "XESP" && f.data.len() >= 8 {
                let parent = u32::from_le_bytes(f.data[0..4].try_into().unwrap());
                let flags = u32::from_le_bytes(f.data[4..8].try_into().unwrap());
                println!("    -> enable parent 0x{parent:08x} flags 0x{flags:08x}");
            }
            if ty == "XLKR" {
                println!("    -> linked ref ({} bytes)", f.data.len());
            }
        }
    }

    // Panel NIF bounds: where does BackPanel01 sit vs the doors?
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut archives: Vec<BsaArchive<File>> = Vec::new();
    let mut paths: Vec<_> = fs::read_dir("test-data")
        .expect("read data dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .map(|n| {
                    let n = n.to_string_lossy().to_lowercase();
                    n.contains("meshes") && n.ends_with(".bsa")
                })
                .unwrap_or(false)
        })
        .collect();
    paths.sort();
    for path in paths {
        if let Ok(archive) = BsaArchive::open(&path) {
            let idx = archives.len();
            for stored in archive.list() {
                index.entry(normalize(stored)).or_insert(idx);
            }
            archives.push(archive);
        }
    }
    for nif_path in [
        "meshes\\Architecture\\Riften\\Interior\\SmallRoomFirst\\RifRmSmSecretCabinetPanel01.nif",
        "meshes\\Architecture\\Riften\\Interior\\SmallRoomFirst\\RifRmSmSecretCabinetDoor01.nif",
    ] {
        let idx = *index.get(&normalize(nif_path)).expect("NIF in BSA index");
        let stored: String = archives[idx]
            .list()
            .into_iter()
            .find(|s| normalize(s) == normalize(nif_path))
            .expect("stored path")
            .to_string();
        let bytes = archives[idx].extract(&stored).expect("extract NIF");
        let nif = NifFile::from_bytes(&bytes).expect("parse NIF");
        println!("\n{nif_path}");
        for mesh in nif.meshes() {
            let mut min = [f32::MAX; 3];
            let mut max = [f32::MIN; 3];
            for v in &mesh.vertices {
                for i in 0..3 {
                    min[i] = min[i].min(v[i]);
                    max[i] = max[i].max(v[i]);
                }
            }
            // Positions are shape-local; apply the composed
            // transform (translation only for bounds here — the
            // shapes hang under unrotated nodes).
            let t = mesh.transform.translation;
            println!(
                "  mesh parent {:?} local bounds X[{:.1},{:.1}] Y[{:.1},{:.1}] Z[{:.1},{:.1}] parent_t=({:.1},{:.1},{:.1})",
                mesh.parent_node, min[0], max[0], min[1], max[1], min[2], max[2],
                t[0], t[1], t[2]
            );
        }
    }
}
