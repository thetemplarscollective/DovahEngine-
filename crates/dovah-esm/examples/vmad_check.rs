//! Parse every VMAD field in a plugin and report what is in them.
//!
//! Usage: `cargo run --example vmad_check -- <path-to-.esm>`
//!
//! Prints the count of records carrying VMAD (by record type), the
//! total attached scripts and properties, the property type
//! distribution, the VMAD versions and object formats seen, and the
//! fragments sections by record type. Any VMAD field that fails to
//! parse is reported with its record type and form ID, and the
//! program exits non-zero — a clean run over Skyrim.esm means the
//! parser handles every script attachment in the game.

use dovah_esm::vmad::Fragments;
use dovah_esm::{Plugin, VmadValue};
use std::collections::BTreeMap;
use std::env;
use std::process::ExitCode;

fn main() -> ExitCode {
    let path = match env::args().nth(1) {
        Some(p) => p,
        None => {
            eprintln!("usage: vmad_check <path-to-.esm>");
            return ExitCode::FAILURE;
        }
    };
    let plugin = match Plugin::open(&path) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("failed to parse {path}: {e}");
            return ExitCode::FAILURE;
        }
    };

    let mut records_with_vmad: usize = 0;
    let mut by_type: BTreeMap<String, usize> = BTreeMap::new();
    let mut scripts: usize = 0;
    let mut properties: usize = 0;
    let mut prop_types: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut versions: BTreeMap<i16, usize> = BTreeMap::new();
    let mut obj_formats: BTreeMap<i16, usize> = BTreeMap::new();
    let mut fragment_records: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut fragment_scripts: usize = 0;
    let mut alias_scripts: usize = 0;
    let mut errors: Vec<(String, u32, String)> = Vec::new();
    let mut sample: Vec<(String, u32, String)> = Vec::new();

    for record in plugin.all_records() {
        let Some(vmad_field) = record.field(b"VMAD") else {
            continue;
        };
        records_with_vmad += 1;
        *by_type.entry(record.type_str()).or_insert(0) += 1;
        match dovah_esm::Vmad::parse(&vmad_field.data, &record.record_type) {
            Ok(vmad) => {
                *versions.entry(vmad.version).or_insert(0) += 1;
                *obj_formats.entry(vmad.obj_format).or_insert(0) += 1;
                scripts += vmad.scripts.len();
                for script in &vmad.scripts {
                    properties += script.properties.len();
                    for prop in &script.properties {
                        let label = match &prop.value {
                            VmadValue::Object { .. } => "object",
                            VmadValue::Str(_) => "string",
                            VmadValue::Int(_) => "int",
                            VmadValue::Float(_) => "float",
                            VmadValue::Bool(_) => "bool",
                            VmadValue::Array(items) => match items.first() {
                                Some(VmadValue::Object { .. }) => "array<object>",
                                Some(VmadValue::Str(_)) => "array<string>",
                                Some(VmadValue::Int(_)) => "array<int>",
                                Some(VmadValue::Float(_)) => "array<float>",
                                Some(VmadValue::Bool(_)) => "array<bool>",
                                Some(VmadValue::Array(_)) => "array<array>",
                                None => "array<empty>",
                            },
                        };
                        *prop_types.entry(label).or_insert(0) += 1;
                    }
                    if sample.len() < 10 {
                        sample.push((record.type_str(), record.form_id, script.name.clone()));
                    }
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
            Err(e) => {
                errors.push((record.type_str(), record.form_id, e.to_string()));
            }
        }
    }

    println!("VMAD check: {path}");
    println!("  records with VMAD: {records_with_vmad}");
    println!("  by record type:");
    for (kind, count) in &by_type {
        println!("    {kind:6} {count}");
    }
    println!("  attached scripts: {scripts}");
    println!("  properties: {properties}");
    println!("  property types:");
    for (kind, count) in &prop_types {
        println!("    {kind:16} {count}");
    }
    println!("  versions: {versions:?}");
    println!("  object formats: {obj_formats:?}");
    println!("  records with fragments: {fragment_records:?}");
    println!("  fragment scripts: {fragment_scripts}");
    println!("  quest-alias scripts: {alias_scripts}");
    println!("  sample scripts:");
    for (kind, form_id, name) in &sample {
        println!("    {kind} 0x{form_id:08x} {name}");
    }
    if errors.is_empty() {
        println!("  parse errors: none");
        ExitCode::SUCCESS
    } else {
        println!("  parse errors: {}", errors.len());
        for (kind, form_id, e) in errors.iter().take(20) {
            println!("    {kind} 0x{form_id:08x}: {e}");
        }
        ExitCode::FAILURE
    }
}
