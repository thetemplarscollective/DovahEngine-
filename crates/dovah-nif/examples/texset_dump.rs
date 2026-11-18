//! Dump each mesh's name + diffuse texture + alpha cutoff for one
//! NIF file, to check what the shader texture sets resolve to.
//! Usage: `cargo run -p dovah-nif --example texset_dump -- <nif-file>...`

use dovah_nif::NifFile;
use std::env;
use std::process::ExitCode;

fn main() -> ExitCode {
    let paths: Vec<String> = env::args().skip(1).collect();
    if paths.is_empty() {
        eprintln!("usage: texset_dump <nif-file>...");
        return ExitCode::from(2);
    }
    for path in &paths {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("{path}: cannot read: {e}");
                continue;
            }
        };
        let nif = match NifFile::from_bytes(&bytes) {
            Ok(n) => n,
            Err(e) => {
                eprintln!("{path}: parse failed: {e:?}");
                continue;
            }
        };
        println!("{path}: {} blocks", nif.blocks.len());
        for mesh in nif.meshes() {
            println!(
                "  mesh {:?} skinned={} verts={} tris={} diffuse={:?} normal={:?} alpha_cutoff={:?}",
                mesh.name,
                mesh.skinned,
                mesh.vertices.len(),
                mesh.indices.len() / 3,
                mesh.textures.diffuse,
                mesh.textures.normal,
                mesh.alpha_cutoff
            );
        }
    }
    ExitCode::SUCCESS
}
