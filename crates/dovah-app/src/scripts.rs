//! Script report (M7 wiring): which Papyrus scripts a cell's
//! records attach, and which of them would run.
//!
//! This is the runtime's first contact with the real game, not a
//! gameplay loop: it collects the VMAD fields of a cell and its
//! placed references, loads whatever `.pex` definitions the data
//! folder has for the attached scripts, creates instances for
//! them (with their VMAD initial properties), and reports which
//! scripts would run — i.e. which instances were created and which
//! `OnInit` events were queued. No scripts are executed here; the
//! VM's scheduler runs them once the game loop owns it.
//!
//! Run with `dovah-app --scripts [--cell EDID] [--esm PATH]
//! [--data DIR]`.

use crate::assets::AssetProvider;
use crate::cell::{find_cell, CellIndex};
use dovah_esm::{Plugin, Vmad};
use dovah_papyrus::Vm;
use dovah_pex::PexFile;

/// One record's script attachments, from its VMAD field.
#[derive(Debug)]
pub struct Attachment {
    /// Form ID of the record carrying the VMAD field.
    pub form_id: u32,
    /// Record type, e.g. `REFR`, `ACHR`, `CELL`.
    pub record_type: String,
    /// Editor ID of the record, when it has one.
    pub edid: String,
    pub vmad: Vmad,
}

/// Collect the VMAD attachments of a cell: the CELL record itself
/// plus every record placed in it. Returns `None` when no CELL
/// with that editor ID exists. A record whose VMAD field fails to
/// parse is skipped (the parser is strict, and the M7 real-data
/// check proved every VMAD field in Skyrim.esm parses — a failure
/// here means a corrupt plugin, not a missing feature).
pub fn collect_attachments(plugin: &Plugin, cell_edid: &str) -> Option<Vec<Attachment>> {
    let cell = find_cell(plugin, cell_edid)?;
    let mut attachments = Vec::new();
    let mut push = |record: &dovah_esm::Record| {
        if let Some(Ok(vmad)) = record.vmad() {
            attachments.push(Attachment {
                form_id: record.form_id,
                record_type: record.type_str(),
                edid: record.edid().unwrap_or_default(),
                vmad,
            });
        }
    };
    push(cell);
    let index = CellIndex::build(plugin);
    for record in index.children_of(cell.form_id) {
        push(record);
    }
    Some(attachments)
}

/// The M7 script report for one cell.
#[derive(Debug)]
pub struct ScriptReport {
    pub cell_edid: String,
    pub cell_form_id: u32,
    /// Records in the cell carrying a VMAD field.
    pub records_with_vmad: usize,
    pub attachments: Vec<Attachment>,
    /// Distinct script names attached anywhere in the cell.
    pub scripts_named: Vec<String>,
    /// Scripts whose `.pex` definition was found in the data
    /// folder and loaded into the VM.
    pub definitions_loaded: Vec<String>,
    /// Scripts named by VMAD but with no `.pex` in the data folder
    /// (the Scripts upload has not landed yet).
    pub definitions_missing: Vec<String>,
    /// Script instances created (one per attached script whose
    /// definition is loaded).
    pub instances_created: usize,
    /// `OnInit` events queued on those instances.
    pub oninit_queued: usize,
    /// Non-fatal problems from instance creation.
    pub warnings: Vec<String>,
}

/// Build the script report for a cell: collect its VMAD
/// attachments, load the `.pex` definitions the data folder has,
/// create the instances, and count what would run.
pub fn build_report(
    plugin: &Plugin,
    cell_edid: &str,
    provider: &mut AssetProvider,
) -> Option<ScriptReport> {
    let cell = find_cell(plugin, cell_edid)?;
    let cell_form_id = cell.form_id;
    let cell_edid = cell.edid().unwrap_or_else(|| cell_edid.to_string());
    let attachments = collect_attachments(plugin, &cell_edid)?;

    let mut scripts_named: Vec<String> = Vec::new();
    for attachment in &attachments {
        for script in &attachment.vmad.scripts {
            if script.status != 3
                && !scripts_named
                    .iter()
                    .any(|n| n.eq_ignore_ascii_case(&script.name))
            {
                scripts_named.push(script.name.clone());
            }
        }
    }
    scripts_named.sort_by_key(|name| name.to_ascii_lowercase());

    let mut vm = Vm::new();
    let mut definitions_loaded: Vec<String> = Vec::new();
    let mut definitions_missing: Vec<String> = Vec::new();
    for name in &scripts_named {
        let path = format!("scripts\\{name}.pex");
        match provider.read_file(&path) {
            Some(bytes) => match PexFile::from_bytes(&bytes) {
                Ok(pex) => {
                    vm.load_pex(&pex);
                    definitions_loaded.push(name.clone());
                }
                Err(e) => {
                    vm.warnings
                        .push(format!("script '{name}': could not parse {path}: {e}"));
                    definitions_missing.push(name.clone());
                }
            },
            None => definitions_missing.push(name.clone()),
        }
    }

    let mut instances_created = 0;
    for attachment in &attachments {
        instances_created += vm
            .attach_scripts_from_vmad(attachment.form_id, &attachment.vmad)
            .len();
    }
    let oninit_queued = vm
        .runnable
        .iter()
        .filter(|stack| stack.event == "OnInit")
        .count();

    Some(ScriptReport {
        records_with_vmad: attachments.len(),
        cell_edid,
        cell_form_id,
        attachments,
        scripts_named,
        definitions_loaded,
        definitions_missing,
        instances_created,
        oninit_queued,
        warnings: vm.warnings,
    })
}

/// Print the report for a human.
pub fn print_report(report: &ScriptReport) {
    println!(
        "scripts for {} (form 0x{:08x}):",
        report.cell_edid, report.cell_form_id
    );
    println!("  records with scripts: {}", report.records_with_vmad);
    println!(
        "  scripts attached: {} distinct",
        report.scripts_named.len()
    );
    println!(
        "  definitions loaded: {} (.pex found in the data folder)",
        report.definitions_loaded.len()
    );
    for name in &report.definitions_loaded {
        println!("    loaded: {name}");
    }
    println!(
        "  definitions missing: {} (no .pex in the data folder)",
        report.definitions_missing.len()
    );
    for name in &report.definitions_missing {
        println!("    missing: {name}");
    }
    println!("  instances created: {}", report.instances_created);
    println!("  OnInit events queued: {}", report.oninit_queued);
    if !report.attachments.is_empty() {
        println!("  per record:");
        for attachment in &report.attachments {
            let scripts: Vec<String> = attachment
                .vmad
                .scripts
                .iter()
                .map(|s| {
                    let loaded = if report
                        .definitions_loaded
                        .iter()
                        .any(|n| n.eq_ignore_ascii_case(&s.name))
                    {
                        "loaded"
                    } else {
                        "no .pex"
                    };
                    let props = if s.properties.len() == 1 {
                        "1 property".to_string()
                    } else {
                        format!("{} properties", s.properties.len())
                    };
                    format!("{} ({props}, {loaded})", s.name)
                })
                .collect();
            let edid = if attachment.edid.is_empty() {
                String::new()
            } else {
                format!(" {}", attachment.edid)
            };
            println!(
                "    {} 0x{:08x}{}: {}",
                attachment.record_type,
                attachment.form_id,
                edid,
                scripts.join(", ")
            );
        }
    }
    if !report.warnings.is_empty() {
        println!("  warnings:");
        for warning in &report.warnings {
            println!("    {warning}");
        }
    }
    if report.definitions_missing.is_empty() && report.instances_created > 0 {
        println!(
            "  -> {} script(s) would run in this cell.",
            report.instances_created
        );
    } else if report.definitions_missing.is_empty() {
        println!("  -> no scripts attached in this cell.");
    } else {
        println!(
            "  -> {} script(s) would run once their .pex files arrive.",
            report.definitions_missing.len()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dovah_esm::vmad::VmadValue;
    use dovah_pex::MAGIC;
    use std::fs;
    use std::path::PathBuf;

    fn field(field_type: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(field_type);
        out.extend_from_slice(&(data.len() as u16).to_le_bytes());
        out.extend_from_slice(data);
        out
    }

    fn zstring_field(field_type: &[u8; 4], text: &str) -> Vec<u8> {
        let mut data = text.as_bytes().to_vec();
        data.push(0);
        field(field_type, &data)
    }

    fn record(record_type: &[u8; 4], form_id: u32, fields: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(record_type);
        out.extend_from_slice(&(fields.len() as u32).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&form_id.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&44u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(fields);
        out
    }

    fn group(label: &[u8; 4], group_type: i32, children: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"GRUP");
        out.extend_from_slice(&(24 + children.len() as u32).to_le_bytes());
        out.extend_from_slice(label);
        out.extend_from_slice(&group_type.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(children);
        out
    }

    fn wstring(out: &mut Vec<u8>, s: &str) {
        out.extend_from_slice(&(s.len() as u16).to_le_bytes());
        out.extend_from_slice(s.as_bytes());
    }

    /// A VMAD field (version 5, objFormat 2) with one script and
    /// its properties.
    fn vmad_bytes(script_name: &str, properties: &[(&str, i32)]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&5i16.to_le_bytes());
        out.extend_from_slice(&2i16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        wstring(&mut out, script_name);
        out.push(0);
        out.extend_from_slice(&(properties.len() as u16).to_le_bytes());
        for (name, value) in properties {
            wstring(&mut out, name);
            out.push(3); // int
            out.push(1); // edited
            out.extend_from_slice(&value.to_le_bytes());
        }
        out
    }

    /// A minimal PEX file: one object named `name`, no functions.
    fn minimal_pex_bytes(name: &str) -> Vec<u8> {
        // String table: "", name, "Script", "" (docstring).
        let mut table = Vec::new();
        table.extend_from_slice(&3u16.to_be_bytes());
        for s in ["", name, "Script"] {
            table.extend_from_slice(&(s.len() as u16).to_be_bytes());
            table.extend_from_slice(s.as_bytes());
        }
        // Object data: parent "Script" (index 2), docstring "" (0),
        // flags 0, auto state "" (0), no variables/properties/states.
        let mut data = Vec::new();
        data.extend_from_slice(&2u16.to_be_bytes());
        data.extend_from_slice(&0u16.to_be_bytes());
        data.extend_from_slice(&0u32.to_be_bytes());
        data.extend_from_slice(&0u16.to_be_bytes());
        data.extend_from_slice(&0u16.to_be_bytes());
        data.extend_from_slice(&0u16.to_be_bytes());
        data.extend_from_slice(&0u16.to_be_bytes());

        let mut out = Vec::new();
        out.extend_from_slice(&MAGIC.to_be_bytes());
        out.push(3);
        out.push(1);
        out.extend_from_slice(&1u16.to_be_bytes());
        out.extend_from_slice(&1_700_000_000u64.to_be_bytes());
        for s in [
            format!("{name}.psc"),
            "jarvis".to_string(),
            "workshop".to_string(),
        ] {
            out.extend_from_slice(&(s.len() as u16).to_be_bytes());
            out.extend_from_slice(s.as_bytes());
        }
        out.extend_from_slice(&table);
        out.push(0); // no debug info
        out.extend_from_slice(&0u16.to_be_bytes()); // user flags
        out.extend_from_slice(&1u16.to_be_bytes()); // one object
        out.extend_from_slice(&1u16.to_be_bytes()); // object name index
        out.extend_from_slice(&(4u32 + data.len() as u32).to_be_bytes());
        out.extend_from_slice(&data);
        out
    }

    /// A plugin with one CELL ("TestCell", 0x100) holding two
    /// REFRs — one with a VMAD field, one without — plus a second
    /// CELL whose REFR must not be collected.
    fn test_plugin() -> Plugin {
        let mut tes4_fields = Vec::new();
        let mut hedr = Vec::new();
        hedr.extend_from_slice(&1.7f32.to_le_bytes());
        hedr.extend_from_slice(&4u32.to_le_bytes());
        hedr.extend_from_slice(&0u32.to_le_bytes());
        tes4_fields.extend_from_slice(&field(b"HEDR", &hedr));

        let mut cell_fields = Vec::new();
        cell_fields.extend_from_slice(&zstring_field(b"EDID", "TestCell"));
        cell_fields.extend_from_slice(&field(b"DATA", &[1]));

        let mut refr_fields = Vec::new();
        refr_fields.extend_from_slice(&zstring_field(b"EDID", "ScriptedDoor"));
        refr_fields.extend_from_slice(&field(
            b"VMAD",
            &vmad_bytes("DoorScript", &[("Health", 100)]),
        ));
        let mut plain_fields = Vec::new();
        plain_fields.extend_from_slice(&zstring_field(b"EDID", "PlainChair"));

        let mut other_cell_fields = Vec::new();
        other_cell_fields.extend_from_slice(&zstring_field(b"EDID", "OtherCell"));
        other_cell_fields.extend_from_slice(&field(b"DATA", &[1]));
        let mut other_refr_fields = Vec::new();
        other_refr_fields.extend_from_slice(&zstring_field(b"EDID", "OtherDoor"));
        other_refr_fields.extend_from_slice(&field(b"VMAD", &vmad_bytes("OtherScript", &[])));

        let mut children = Vec::new();
        children.extend_from_slice(&record(b"REFR", 0x1001, &refr_fields));
        children.extend_from_slice(&record(b"REFR", 0x1002, &plain_fields));
        let mut other_children = Vec::new();
        other_children.extend_from_slice(&record(b"REFR", 0x2001, &other_refr_fields));

        let mut cell_children = Vec::new();
        cell_children.extend_from_slice(&record(b"CELL", 0x100, &cell_fields));
        cell_children.extend_from_slice(&group(&0x100u32.to_le_bytes(), 9, &children));
        cell_children.extend_from_slice(&record(b"CELL", 0x200, &other_cell_fields));
        cell_children.extend_from_slice(&group(&0x200u32.to_le_bytes(), 9, &other_children));

        let mut bytes = Vec::new();
        bytes.extend_from_slice(&record(b"TES4", 0, &tes4_fields));
        bytes.extend_from_slice(&group(b"CELL", 0, &cell_children));
        Plugin::from_bytes(&bytes).unwrap()
    }

    fn temp_data_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("scripts")).unwrap();
        dir
    }

    #[test]
    fn collects_only_the_cells_own_vmad() {
        let plugin = test_plugin();
        let attachments = collect_attachments(&plugin, "TestCell").unwrap();
        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0].form_id, 0x1001);
        assert_eq!(attachments[0].record_type, "REFR");
        assert_eq!(attachments[0].edid, "ScriptedDoor");
        assert_eq!(attachments[0].vmad.scripts.len(), 1);
        assert_eq!(attachments[0].vmad.scripts[0].name, "DoorScript");
        assert_eq!(attachments[0].vmad.scripts[0].properties.len(), 1);
        assert_eq!(
            attachments[0].vmad.scripts[0].properties[0].value,
            VmadValue::Int(100)
        );
        assert!(collect_attachments(&plugin, "NoSuchCell").is_none());
        // Case-insensitive cell lookup.
        assert_eq!(collect_attachments(&plugin, "testcell").unwrap().len(), 1);
    }

    #[test]
    fn report_with_no_pex_files() {
        let plugin = test_plugin();
        let dir = temp_data_dir("dovah-app-test-scripts-empty");
        let mut provider = AssetProvider::new(&dir);
        let report = build_report(&plugin, "TestCell", &mut provider).unwrap();
        assert_eq!(report.records_with_vmad, 1);
        assert_eq!(report.scripts_named, vec!["DoorScript".to_string()]);
        assert!(report.definitions_loaded.is_empty());
        assert_eq!(report.definitions_missing, vec!["DoorScript".to_string()]);
        assert_eq!(report.instances_created, 0);
        assert_eq!(report.oninit_queued, 0);
        assert_eq!(report.warnings.len(), 1);
        assert!(report.warnings[0].contains("DoorScript"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn report_with_a_pex_file_creates_an_instance() {
        let plugin = test_plugin();
        let dir = temp_data_dir("dovah-app-test-scripts-loaded");
        fs::write(
            dir.join("scripts").join("DoorScript.pex"),
            minimal_pex_bytes("DoorScript"),
        )
        .unwrap();
        let mut provider = AssetProvider::new(&dir);
        let report = build_report(&plugin, "TestCell", &mut provider).unwrap();
        assert_eq!(report.definitions_loaded, vec!["DoorScript".to_string()]);
        assert!(report.definitions_missing.is_empty());
        assert_eq!(report.instances_created, 1);
        // The minimal PEX defines no OnInit, so nothing is queued.
        assert_eq!(report.oninit_queued, 0);
        // The minimal PEX declares no `Health` property, so the
        // VMAD initial value is warned about, not applied — the
        // instance itself is still created.
        assert_eq!(report.warnings.len(), 1);
        assert!(report.warnings[0].contains("Health"));
        let _ = fs::remove_dir_all(&dir);
    }
}
