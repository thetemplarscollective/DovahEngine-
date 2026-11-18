//! Verify the inn door pair: the interior door (0x00013419) XTELs
//! to the exterior door (0x00013424); the exterior door must XTEL
//! back to the interior door. Prints both XTELs (dest ref, pos,
//! rot) — the two spawn points for the M9 door transitions.

use dovah_esm::Plugin;

const ESM_PATH: &str = "test-data/Skyrim.esm";

fn parse_xtel(data: &[u8]) -> Option<(u32, [f32; 3], [f32; 3], u32)> {
    if data.len() != 32 {
        return None;
    }
    let u = |i: usize| u32::from_le_bytes(data[i * 4..i * 4 + 4].try_into().unwrap());
    let f = |i: usize| f32::from_le_bytes(data[i * 4..i * 4 + 4].try_into().unwrap());
    Some((u(0), [f(1), f(2), f(3)], [f(4), f(5), f(6)], u(7)))
}

fn parse_placement(data: &[u8]) -> Option<([f32; 3], [f32; 3])> {
    if data.len() != 24 {
        return None;
    }
    let f = |i: usize| f32::from_le_bytes(data[i * 4..i * 4 + 4].try_into().unwrap());
    Some(([f(0), f(1), f(2)], [f(3), f(4), f(5)]))
}

fn main() {
    let plugin = Plugin::open(ESM_PATH).expect("open Skyrim.esm");
    for ref_id in [0x00013419u32, 0x00013424u32] {
        let Some(r) = plugin.find(ref_id) else {
            println!("ref 0x{ref_id:08x}: NOT FOUND");
            continue;
        };
        let (pos, rot) = r
            .field(b"DATA")
            .and_then(|f| parse_placement(&f.data))
            .unwrap_or(([f32::NAN; 3], [f32::NAN; 3]));
        println!(
            "ref 0x{ref_id:08x} ({}): pos=({:.1},{:.1},{:.1}) rot=({:.3},{:.3},{:.3})",
            r.edid().unwrap_or_default(),
            pos[0],
            pos[1],
            pos[2],
            rot[0],
            rot[1],
            rot[2]
        );
        match r.field(b"XTEL").and_then(|f| parse_xtel(&f.data)) {
            Some((dest, dpos, drot, tail)) => {
                println!("  XTEL -> dest ref 0x{dest:08x} pos=({:.1},{:.1},{:.1}) rot=({:.3},{:.3},{:.3}) tail=0x{tail:08x}",
                    dpos[0], dpos[1], dpos[2], drot[0], drot[1], drot[2]);
            }
            None => println!(
                "  no XTEL field (len {:?})",
                r.field(b"XTEL").map(|f| f.data.len())
            ),
        }
    }
}
