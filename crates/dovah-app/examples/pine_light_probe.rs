//! Exterior follow-up probe: dump the DATA behind the pine
//! darkness and the waterfall-fog white plane — per-shape vertex
//! colors, normals, alpha properties, shader blocks, and the
//! diffuse textures' mean brightness (opaque pixels only, so a
//! mostly-transparent needle texture is not averaged to black
//! by its empty background).
//!
//! Usage: cargo run -p dovah-app --example pine_light_probe --
//!        [nif-path ...]
//! Default NIFs: TreePineForest01/02, TreePineForestSnowL02,
//! FXWaterfallSkirtTallFront.

use dovah_bsa::BsaArchive;
use dovah_nif::{Block, NifFile};
use std::collections::HashMap;
use std::fs::{self, File};

fn normalize(path: &str) -> String {
    path.replace('\\', "/").to_lowercase()
}

struct Index {
    archives: Vec<BsaArchive<File>>,
    map: HashMap<String, usize>,
}

impl Index {
    fn open() -> Self {
        let mut archives = Vec::new();
        let mut map = HashMap::new();
        let mut paths: Vec<_> = fs::read_dir("test-data")
            .expect("read data dir")
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.extension()
                    .map(|e| e.eq_ignore_ascii_case("bsa"))
                    .unwrap_or(false)
            })
            .collect();
        paths.sort();
        for path in paths {
            match BsaArchive::open(&path) {
                Ok(archive) => {
                    let idx = archives.len();
                    for stored in archive.list() {
                        map.entry(normalize(stored)).or_insert(idx);
                    }
                    archives.push(archive);
                }
                Err(e) => eprintln!("warning: could not open {}: {e}", path.display()),
            }
        }
        Index { archives, map }
    }

    fn read(&mut self, path: &str) -> Option<Vec<u8>> {
        let idx = *self.map.get(&normalize(path))?;
        let stored: String = self.archives[idx]
            .list()
            .into_iter()
            .find(|s| normalize(s) == normalize(path))?
            .to_string();
        self.archives[idx].extract(&stored).ok()
    }
}

fn tex_stats(index: &mut Index, path: &str) {
    let Some(bytes) = index.read(path) else {
        println!("      texture {path}: NOT FOUND");
        return;
    };
    let Ok(dds) = dovah_dds::DdsImage::from_bytes(&bytes) else {
        println!("      texture {path}: parse failed");
        return;
    };
    let Ok(img) = dds.decode(0) else {
        println!("      texture {path}: decode failed");
        return;
    };
    let n = (img.width * img.height) as usize;
    let mut sr = 0u64;
    let mut sg = 0u64;
    let mut sb = 0u64;
    let mut sa = 0u64;
    let mut or = 0u64;
    let mut og = 0u64;
    let mut ob = 0u64;
    let mut opaque = 0u64;
    for p in 0..n {
        let r = img.data[p * 4] as u64;
        let g = img.data[p * 4 + 1] as u64;
        let b = img.data[p * 4 + 2] as u64;
        let a = img.data[p * 4 + 3] as u64;
        sr += r;
        sg += g;
        sb += b;
        sa += a;
        if a >= 128 {
            or += r;
            og += g;
            ob += b;
            opaque += 1;
        }
    }
    let nf = n as f64;
    let of = opaque.max(1) as f64;
    // G-R distribution among opaque pixels: is the green in a
    // distinct needle cluster, or is the texture uniformly grey?
    let mut gr: Vec<i32> = Vec::new();
    for p in 0..n {
        let a = img.data[p * 4 + 3] as i32;
        if a >= 128 {
            gr.push(img.data[p * 4 + 1] as i32 - img.data[p * 4] as i32);
        }
    }
    gr.sort();
    let pct = |q: f64| -> i32 {
        if gr.is_empty() {
            0
        } else {
            gr[((gr.len() - 1) as f64 * q) as usize]
        }
    };
    let green_frac = gr.iter().filter(|&&v| v >= 13).count() as f64 / of;
    println!(
        "      texture {path}: {}x{} mean RGBA=({:.0},{:.0},{:.0},{:.0}) opaque {:.0}% mean RGB=({:.0},{:.0},{:.0}) G-R p10={} p50={} p90={} frac(G-R>=13)={:.2}",
        img.width,
        img.height,
        sr as f64 / nf,
        sg as f64 / nf,
        sb as f64 / nf,
        sa as f64 / nf,
        100.0 * opaque as f64 / nf,
        or as f64 / of,
        og as f64 / of,
        ob as f64 / of,
        pct(0.10),
        pct(0.50),
        pct(0.90),
        green_frac,
    );
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        args = vec![
            "meshes\\landscape\\trees\\treePineForest01.nif".to_string(),
            "meshes\\landscape\\trees\\treePineForest02.nif".to_string(),
            "meshes\\landscape\\trees\\treePineForestSnowL02.nif".to_string(),
            "meshes\\effects\\fxwaterfallskirttallfront.nif".to_string(),
        ];
    }
    let mut index = Index::open();
    for want in &args {
        let Some(bytes) = index.read(want) else {
            println!("{want}: NOT IN ARCHIVES");
            continue;
        };
        let nif = match NifFile::from_bytes(&bytes) {
            Ok(n) => n,
            Err(e) => {
                println!("{want}: parse failed: {e:?}");
                continue;
            }
        };
        println!("=== {want} ({} blocks) ===", nif.blocks.len());
        for (i, block) in nif.blocks.iter().enumerate() {
            match block {
                Block::AlphaProperty(p) => println!(
                    "  [{i}] NiAlphaProperty flags=0x{:04x} threshold={}",
                    p.flags, p.threshold
                ),
                Block::LightingShaderProperty(s) => println!(
                    "  [{i}] BSLightingShaderProperty '{}' type={} texset={} alpha={:.2} emissive={:?}x{:.2} spec={:?}x{:.2} gloss={:.2}",
                    s.net.name, s.shader_type, s.texture_set, s.alpha,
                    s.emissive_color, s.emissive_multiple,
                    s.specular_color, s.specular_strength, s.glossiness
                ),
                Block::ShaderTextureSet(t) => {
                    println!("  [{i}] BSShaderTextureSet {:?}", t.textures)
                }
                Block::MaterialProperty(m) => println!(
                    "  [{i}] NiMaterialProperty spec={:?} emis={:?} gloss={:.2} alpha={:.2} emis_mult={:.2}",
                    m.specular_color, m.emissive_color, m.glossiness, m.alpha, m.emissive_mult
                ),
                Block::Unknown { type_name, .. } => {
                    println!("  [{i}] Unknown {type_name}")
                }
                other => println!("  [{i}] {}", other.type_name()),
            }
        }
        for mesh in nif.meshes() {
            let nv = mesh.vertex_colors.len();
            let (mut cr, mut cg, mut cb, mut ca) = (0.0f64, 0.0, 0.0, 0.0);
            let (mut mnr, mut mng, mut mnb, mut mna) = (1.0f64, 1.0f64, 1.0f64, 1.0f64);
            for c in &mesh.vertex_colors {
                cr += c[0] as f64;
                cg += c[1] as f64;
                cb += c[2] as f64;
                ca += c[3] as f64;
                mnr = mnr.min(c[0] as f64);
                mng = mng.min(c[1] as f64);
                mnb = mnb.min(c[2] as f64);
                mna = mna.min(c[3] as f64);
            }
            let nf = nv.max(1) as f64;
            // Normal stats: mean direction + mean |nz| (Skyrim +Z
            // is up; cards with horizontal normals have |nz| ~ 0).
            let nn = mesh.normals.len();
            let (mut nx, mut ny, mut nz, mut anz) = (0.0f64, 0.0, 0.0, 0.0);
            for n in &mesh.normals {
                nx += n[0] as f64;
                ny += n[1] as f64;
                nz += n[2] as f64;
                anz += (n[2] as f64).abs();
            }
            let nfn = nn.max(1) as f64;
            println!(
                "  mesh '{}' parent='{}' verts={} tris={} skinned={} cutoff={:?} blend={:?} unlit={}",
                mesh.name,
                mesh.parent_node,
                mesh.vertices.len(),
                mesh.indices.len() / 3,
                mesh.skinned,
                mesh.alpha_cutoff,
                mesh.alpha_blend,
                mesh.unlit,
            );
            // UV range: a fog sheet with no UVs (or a degenerate
            // range) samples one texel — flat color, no clouds.
            // And where the transparent vertices sit: at the
            // silhouette edge (soft fade) or scattered.
            if !mesh.uvs.is_empty() {
                let (mut umin, mut umax, mut vmin, mut vmax) =
                    (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
                for uv in &mesh.uvs {
                    umin = umin.min(uv[0]);
                    umax = umax.max(uv[0]);
                    vmin = vmin.min(uv[1]);
                    vmax = vmax.max(uv[1]);
                }
                println!("    uv: U [{umin:.2}, {umax:.2}] V [{vmin:.2}, {vmax:.2}]");
            } else {
                println!("    uv: none");
            }
            if nv > 0 {
                // Vertex positions at alpha 0 vs the mesh bounds:
                // are the transparent verts on the rim?
                let (mut xmin, mut xmax, mut zmin, mut zmax) =
                    (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
                for v in &mesh.vertices {
                    xmin = xmin.min(v[0]);
                    xmax = xmax.max(v[0]);
                    zmin = zmin.min(v[2]);
                    zmax = zmax.max(v[2]);
                }
                let mut clear: Vec<[f32; 3]> = Vec::new();
                for (i, c) in mesh.vertex_colors.iter().enumerate() {
                    if c[3] < 0.05 {
                        clear.push(mesh.vertices[i]);
                    }
                }
                println!(
                    "    bounds X [{xmin:.0}, {xmax:.0}] Z [{zmin:.0}, {zmax:.0}] (units); alpha<0.05 verts: {} of {nv}",
                    clear.len()
                );
                for v in clear.iter().take(6) {
                    println!(
                        "      clear vert at ({:.0}, {:.0}, {:.0})",
                        v[0], v[1], v[2]
                    );
                }
            }
            if nv > 0 {
                println!(
                    "    vcolor mean=({:.2},{:.2},{:.2},{:.2}) min=({:.2},{:.2},{:.2},{:.2})",
                    cr / nf,
                    cg / nf,
                    cb / nf,
                    ca / nf,
                    mnr,
                    mng,
                    mnb,
                    mna
                );
            } else {
                println!("    vcolor: none");
            }
            if nn > 0 {
                println!(
                    "    normals mean=({:.2},{:.2},{:.2}) mean|nz|={:.2}",
                    nx / nfn,
                    ny / nfn,
                    nz / nfn,
                    anz / nfn
                );
            }
            if let Some(diffuse) = &mesh.textures.diffuse {
                tex_stats(&mut index, diffuse);
            } else {
                println!("    texture: none");
            }
        }
    }
}
