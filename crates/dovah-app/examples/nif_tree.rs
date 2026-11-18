//! M11 probe: dump a NIF's block tree (types, names, AvObject
//! transforms, node children) to find where a shape's parent
//! transforms live. Used to check whether FarmhouseLDoor01.nif's
//! 'Door:7' panel hangs under a +88u parent node that the
//! exterior audit's Part B walk missed.
//!
//! Usage: cargo run -p dovah-app --example nif_tree -- <nif-path>
//! e.g. meshes\Architecture\Farmhouse\FarmhouseLDoor01.nif

use dovah_bsa::BsaArchive;
use dovah_nif::{Block, NifFile};
use std::collections::HashMap;
use std::fs::{self, File};

fn normalize(path: &str) -> String {
    path.replace('\\', "/").to_lowercase()
}

fn main() {
    let want = std::env::args().nth(1).expect("NIF path argument");
    // Open the Meshes BSAs and find the file.
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut archives: Vec<BsaArchive<File>> = Vec::new();
    let mut paths: Vec<_> = fs::read_dir("test-data")
        .expect("read data dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.to_lowercase().contains("meshes"))
                .unwrap_or(false)
        })
        .collect();
    paths.sort();
    for path in paths {
        match BsaArchive::open(&path) {
            Ok(archive) => {
                let idx = archives.len();
                for stored in archive.list() {
                    index.entry(normalize(stored)).or_insert(idx);
                }
                archives.push(archive);
            }
            Err(e) => eprintln!("warning: could not open {}: {e}", path.display()),
        }
    }
    let idx = *index
        .get(&normalize(&want))
        .expect("NIF not in Meshes BSAs");
    let stored: String = archives[idx]
        .list()
        .into_iter()
        .find(|s| normalize(s) == normalize(&want))
        .expect("stored name")
        .to_string();
    let bytes = archives[idx].extract(&stored).expect("extract NIF");
    let nif = NifFile::from_bytes(&bytes).expect("parse NIF");
    println!("NIF: {want}");
    println!(
        "  version 0x{:08x} bs_version {} blocks: {} roots: {:?}",
        nif.version,
        nif.bs_version,
        nif.blocks.len(),
        nif.root_indices
    );
    for (i, block) in nif.blocks.iter().enumerate() {
        match block {
            Block::Node(n) => println!(
                "  [{i}] NiNode '{}' t=({:.1},{:.1},{:.1}) scale={:.3} children={:?}",
                n.av.net.name,
                n.av.translation[0],
                n.av.translation[1],
                n.av.translation[2],
                n.av.scale,
                n.children
            ),
            Block::SwitchNode(s) => println!(
                "  [{i}] NiSwitchNode '{}' t=({:.1},{:.1},{:.1}) scale={:.3} children={:?} active={}",
                s.node.av.net.name,
                s.node.av.translation[0],
                s.node.av.translation[1],
                s.node.av.translation[2],
                s.node.av.scale,
                s.node.children,
                s.active_child
            ),
            Block::TriShape(s) => println!(
                "  [{i}] NiTriShape '{}' t=({:.1},{:.1},{:.1}) scale={:.3}",
                s.geometry.av.net.name,
                s.geometry.av.translation[0],
                s.geometry.av.translation[1],
                s.geometry.av.translation[2],
                s.geometry.av.scale
            ),
            Block::TriStrips(s) => println!(
                "  [{i}] NiTriStrips '{}' t=({:.1},{:.1},{:.1}) scale={:.3}",
                s.geometry.av.net.name,
                s.geometry.av.translation[0],
                s.geometry.av.translation[1],
                s.geometry.av.translation[2],
                s.geometry.av.scale
            ),
            Block::BsLodTriShape(s) => println!(
                "  [{i}] BSLodTriShape '{}' t=({:.1},{:.1},{:.1}) scale={:.3}",
                s.geometry.av.net.name,
                s.geometry.av.translation[0],
                s.geometry.av.translation[1],
                s.geometry.av.translation[2],
                s.geometry.av.scale
            ),
            Block::BsTriShape(s) => println!(
                "  [{i}] BSTriShape '{}' t=({:.1},{:.1},{:.1}) scale={:.3}",
                s.av.net.name,
                s.av.translation[0],
                s.av.translation[1],
                s.av.translation[2],
                s.av.scale
            ),
            Block::BsDynamicTriShape(d) => println!(
                "  [{i}] BSDynamicTriShape '{}' t=({:.1},{:.1},{:.1}) scale={:.3}",
                d.shape.av.net.name,
                d.shape.av.translation[0],
                d.shape.av.translation[1],
                d.shape.av.translation[2],
                d.shape.av.scale
            ),
            other => println!("  [{i}] {}", other.type_name()),
        }
    }
    // Which shapes does meshes() return, and what are their local transforms?
    println!("  meshes():");
    for mesh in nif.meshes() {
        println!(
            "    '{}' verts={} t=({:.1},{:.1},{:.1}) scale={:.3}",
            mesh.name,
            mesh.vertices.len(),
            mesh.transform.translation[0],
            mesh.transform.translation[1],
            mesh.transform.translation[2],
            mesh.transform.scale
        );
    }
}
