//! Arrow-axis probe: print the per-shape AABB (NIF space) of the
//! flight NIFs, so the archery system knows the arrow's long axis
//! and which end is the tip (the trail sits behind the arrow).
//!
//! Usage: cargo run -p dovah-nif --example arrow_axis_probe

use dovah_bsa::BsaArchive;
use dovah_nif::NifFile;

const NIFS: &[&str] = &[
    "meshes/weapons/iron/ironarrowflight.nif",
    "meshes/weapons/steel/steelarrowflight.nif",
    "meshes/weapons/orcish/orcisharrowprojectile.nif",
];

fn main() {
    let mut archives = vec![];
    for name in ["Skyrim_-_Meshes0.bsa", "Skyrim_-_Meshes1.bsa"] {
        let path = format!("test-data/{name}");
        match BsaArchive::open(&path) {
            Ok(a) => archives.push(a),
            Err(e) => eprintln!("warning: {path}: {e}"),
        }
    }
    for nif_path in NIFS {
        let mut bytes = None;
        for a in &mut archives {
            // Stored names are lower-case with forward slashes.
            let want = nif_path.to_lowercase();
            let stored = a
                .list()
                .into_iter()
                .find(|s| *s == want)
                .map(|s| s.to_string());
            if let Some(stored) = stored {
                bytes = a.extract(&stored).ok();
                break;
            }
        }
        let Some(bytes) = bytes else {
            println!("{nif_path}: NOT FOUND in Meshes BSAs");
            continue;
        };
        let nif = NifFile::from_bytes(&bytes).expect("parse nif");
        println!("=== {nif_path} ===");
        for mesh in nif.meshes() {
            if mesh.vertices.is_empty() {
                continue;
            }
            let mut min = [f32::MAX; 3];
            let mut max = [f32::MIN; 3];
            for p in &mesh.vertices {
                for i in 0..3 {
                    min[i] = min[i].min(p[i]);
                    max[i] = max[i].max(p[i]);
                }
            }
            println!(
                "  shape '{}' verts={} AABB min=({:.1},{:.1},{:.1}) max=({:.1},{:.1},{:.1}) extents=({:.1},{:.1},{:.1})",
                mesh.name,
                mesh.vertices.len(),
                min[0],
                min[1],
                min[2],
                max[0],
                max[1],
                max[2],
                max[0] - min[0],
                max[1] - min[1],
                max[2] - min[2]
            );
            // Tip-vs-fletching: the tip is a POINT (vertices converge),
            // the fletching spreads. Compare the X/Z spread of the
            // extreme low-Y vs high-Y vertex bands.
            if mesh.name.to_lowercase().contains("arrow") {
                let mut lo = (f32::MAX, f32::MIN, f32::MAX, f32::MIN, 0usize);
                let mut hi = (f32::MAX, f32::MIN, f32::MAX, f32::MIN, 0usize);
                for p in &mesh.vertices {
                    if p[1] < 3.0 {
                        lo.0 = lo.0.min(p[0]);
                        lo.1 = lo.1.max(p[0]);
                        lo.2 = lo.2.min(p[2]);
                        lo.3 = lo.3.max(p[2]);
                        lo.4 += 1;
                    }
                    if p[1] > 55.0 {
                        hi.0 = hi.0.min(p[0]);
                        hi.1 = hi.1.max(p[0]);
                        hi.2 = hi.2.min(p[2]);
                        hi.3 = hi.3.max(p[2]);
                        hi.4 += 1;
                    }
                }
                println!(
                    "    y<3: n={} x[{:.2},{:.2}] z[{:.2},{:.2}] | y>55: n={} x[{:.2},{:.2}] z[{:.2},{:.2}]",
                    lo.4, lo.0, lo.1, lo.2, lo.3, hi.4, hi.0, hi.1, hi.2, hi.3
                );
            }
        }
    }
}
