//! Real-data check for the VMAD parser: parse every VMAD field in
//! Dylan's Skyrim.esm and verify the counts. Skips (passes) when the
//! file is not present — the synthetic tests in `vmad_roundtrip.rs`
//! are the ones that run everywhere.
//!
//! The pinned counts come from `vmad_check` (see
//! `crates/dovah-esm/examples/vmad_check.rs`); they are exact values
//! from the real file, not estimates.

use dovah_esm::vmad::Fragments;
use dovah_esm::{Plugin, VmadValue};
use std::collections::BTreeMap;
use std::path::PathBuf;

fn skyrim_esm() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("test-data")
        .join("Skyrim.esm")
}

#[test]
fn every_vmad_field_in_skyrim_esm_parses() {
    let path = skyrim_esm();
    if !path.exists() {
        eprintln!(
            "skipping real-data VMAD check: {} not present",
            path.display()
        );
        return;
    }
    let plugin = Plugin::open(&path).expect("Skyrim.esm should parse");

    let mut records_with_vmad: usize = 0;
    let mut scripts: usize = 0;
    let mut properties: usize = 0;
    let mut versions: BTreeMap<i16, usize> = BTreeMap::new();
    let mut obj_formats: BTreeMap<i16, usize> = BTreeMap::new();
    let mut fragment_records: BTreeMap<&str, usize> = BTreeMap::new();
    let mut fragment_scripts: usize = 0;
    let mut alias_scripts: usize = 0;
    let mut errors: Vec<(String, u32, String)> = Vec::new();

    for record in plugin.all_records() {
        let Some(vmad_field) = record.field(b"VMAD") else {
            continue;
        };
        records_with_vmad += 1;
        match dovah_esm::Vmad::parse(&vmad_field.data, &record.record_type) {
            Ok(vmad) => {
                *versions.entry(vmad.version).or_insert(0) += 1;
                *obj_formats.entry(vmad.obj_format).or_insert(0) += 1;
                scripts += vmad.scripts.len();
                for script in &vmad.scripts {
                    properties += script.properties.len();
                }
                if let Some(frag) = &vmad.fragments {
                    match frag {
                        Fragments::Flagged { fragments, .. } => {
                            *fragment_records.entry("INFO/PACK").or_insert(0) += 1;
                            fragment_scripts += fragments.len();
                        }
                        Fragments::Perk { fragments, .. } => {
                            *fragment_records.entry("PERK").or_insert(0) += 1;
                            fragment_scripts += fragments.len();
                        }
                        Fragments::Quest {
                            fragments, aliases, ..
                        } => {
                            *fragment_records.entry("QUST").or_insert(0) += 1;
                            fragment_scripts += fragments.len();
                            for alias in aliases {
                                alias_scripts += alias.scripts.len();
                            }
                        }
                        Fragments::Scene {
                            begin_end, phases, ..
                        } => {
                            *fragment_records.entry("SCEN").or_insert(0) += 1;
                            fragment_scripts += begin_end.len() + phases.len();
                        }
                    }
                }
            }
            Err(e) => errors.push((record.type_str(), record.form_id, e.to_string())),
        }
    }

    assert!(
        errors.is_empty(),
        "VMAD parse errors (first 10): {:?}",
        &errors[..errors.len().min(10)]
    );
    assert_eq!(records_with_vmad, 16133);
    assert_eq!(scripts, 17407);
    assert_eq!(properties, 46493);
    assert_eq!(versions.get(&4), Some(&2561));
    assert_eq!(versions.get(&5), Some(&13572));
    assert_eq!(obj_formats.get(&1), Some(&2705));
    assert_eq!(obj_formats.get(&2), Some(&13428));
    assert_eq!(fragment_records.get("INFO/PACK"), Some(&5570));
    assert_eq!(fragment_records.get("PERK"), Some(&5));
    assert_eq!(fragment_records.get("QUST"), Some(&856));
    assert_eq!(fragment_records.get("SCEN"), Some(&557));
    assert_eq!(fragment_scripts, 12355);
    assert_eq!(alias_scripts, 2529);

    // Spot check on the same parse (parsing the plugin twice would
    // double the test's ~15 s): MGEF 0x0010fc14, the first VMAD
    // field found by the byte probe.
    let record = plugin
        .find(0x0010_FC14)
        .expect("MGEF 0x0010fc14 should exist");
    assert_eq!(record.type_str(), "MGEF");
    let vmad = record.vmad().expect("MGEF should have VMAD").unwrap();
    assert_eq!(vmad.version, 5);
    assert_eq!(vmad.obj_format, 2);
    assert_eq!(vmad.scripts.len(), 1);
    let script = &vmad.scripts[0];
    assert_eq!(script.name, "DragonPriestUltraMaskEffect");
    assert_eq!(script.status, 0);
    assert_eq!(script.properties.len(), 9);
    let chance = script
        .properties
        .iter()
        .find(|p| p.name == "effectChance")
        .expect("effectChance property");
    assert_eq!(chance.value, VmadValue::Float(0.15));
    let cloak = script
        .properties
        .iter()
        .find(|p| p.name == "flameCloak")
        .expect("flameCloak property");
    assert_eq!(
        cloak.value,
        VmadValue::Object {
            form_id: 0x0010_FC17,
            alias_id: None
        }
    );
}
