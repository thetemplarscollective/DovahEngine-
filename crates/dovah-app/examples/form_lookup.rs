//! Look up base-record form IDs by EDID substring, for building
//! `--give` lists and other proof hooks. Usage:
//! `cargo run -p dovah-app --example form_lookup -- IronDagger FoodBread`
//! Prints one line per match: type, EDID, form ID.

use dovah_esm::Plugin;
use std::env;

const ESM_PATH: &str = "test-data/Skyrim.esm";

fn main() {
    let needles: Vec<String> = env::args().skip(1).map(|s| s.to_lowercase()).collect();
    if needles.is_empty() {
        eprintln!("usage: form_lookup <edid-substring> [substring...]");
        return;
    }
    let plugin = Plugin::open(ESM_PATH).unwrap();
    for record in plugin.all_records() {
        let Some(edid) = record.edid() else {
            continue;
        };
        let lower = edid.to_lowercase();
        if needles.iter().any(|n| lower.contains(n.as_str())) {
            println!("{} {} 0x{:08x}", record.type_str(), edid, record.form_id);
        }
    }
}
