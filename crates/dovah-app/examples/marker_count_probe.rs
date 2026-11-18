//! UI M2 follow-up: settle the 358-vs-356 marker count.
//! Lists every XMRK REFR in the six world-map cells (Tamriel
//! persistent + the five city persistent cells) via the REAL
//! `CellIndex` the menu uses, so the two extra markers get
//! names instead of a guess.

use dovah_app::cell::CellIndex;
use dovah_esm::Plugin;

fn main() {
    let bytes = std::fs::read("test-data/Skyrim.esm").expect("Skyrim.esm");
    let plugin = Plugin::from_bytes(&bytes).expect("parse");
    let index = CellIndex::build(&plugin);
    let cells = [
        (0x0000_0d74, "Tamriel persistent"),
        (0x0001_a270, "Whiterun persistent"),
        (0x0001_6be0, "Riften persistent"),
        (0x0001_691f, "Windhelm persistent"),
        (0x0001_6e02, "Markarth persistent"),
        (0x0003_7ee6, "Solitude persistent"),
    ];
    let mut total = 0;
    for (cell, label) in cells {
        let markers: Vec<_> = index
            .children_of(cell)
            .iter()
            .filter(|r| r.type_str() == "REFR" && r.field(b"XMRK").is_some())
            .collect();
        println!("cell 0x{cell:08x} ({label}): {} XMRK", markers.len());
        total += markers.len();
        for r in &markers {
            let tnam = r
                .field(b"TNAM")
                .and_then(|f| f.data.get(0..2))
                .map(|b| u16::from_le_bytes(b.try_into().unwrap()));
            let has_data = r
                .field(b"DATA")
                .map(|f| f.data.len() >= 12)
                .unwrap_or(false);
            println!(
                "  0x{:08x} edid={} tnam={tnam:?} data12={has_data}",
                r.form_id,
                r.edid().unwrap_or_default()
            );
        }
    }
    println!("TOTAL: {total}");
}
