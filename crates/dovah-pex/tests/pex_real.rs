//! Real-data check for the PEX parser (M7.1): parse every .pex
//! file extracted from Dylan's Skyrim - Misc.bsa and verify the
//! counts. Skips (passes) when the scripts directory is not
//! present — the synthetic tests in `pex_roundtrip.rs` are the
//! ones that run everywhere.
//!
//! The pinned counts come from `pex_sweep` (see
//! `crates/dovah-pex/examples/pex_sweep.rs`); they are exact
//! values from the real files, not estimates. One file,
//! `tif__000c4428.pex`, failed to extract on a disk-quota error
//! and is missing from the set — when it is re-extracted, the
//! file count pin becomes 13,495.

use dovah_pex::PexFile;
use std::fs;
use std::path::PathBuf;

fn scripts_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("test-data")
        .join("misc")
        .join("scripts")
}

#[test]
fn every_real_pex_file_parses() {
    let dir = scripts_dir();
    if !dir.join("cwscript.pex").exists() {
        eprintln!(
            "skipping real-data PEX check: {} not present",
            dir.display()
        );
        return;
    }
    let mut paths: Vec<PathBuf> = Vec::new();
    for entry in fs::read_dir(&dir).expect("read scripts dir") {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("pex"))
        {
            paths.push(path);
        }
    }
    paths.sort();
    assert_eq!(
        paths.len(),
        13_494,
        "pex file count changed — update the pins"
    );

    let mut objects = 0usize;
    let mut functions = 0usize;
    let mut instructions = 0usize;
    let mut natives = 0usize;
    for path in &paths {
        let bytes = fs::read(path).expect("read pex");
        let file =
            PexFile::from_bytes(&bytes).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        for object in &file.objects {
            objects += 1;
            for state in &object.states {
                for named in &state.functions {
                    functions += 1;
                    if named.function.is_native() {
                        natives += 1;
                    }
                    instructions += named.function.instructions.len();
                }
            }
            // Property get/set handlers are functions too (the
            // sweep counts them the same way).
            for property in &object.properties {
                for handler in property
                    .read_handler
                    .iter()
                    .chain(property.write_handler.iter())
                {
                    functions += 1;
                    if handler.is_native() {
                        natives += 1;
                    }
                    instructions += handler.instructions.len();
                }
            }
        }
    }
    assert_eq!(objects, 13_494);
    assert_eq!(functions, 52_812);
    assert_eq!(natives, 674);
    assert_eq!(instructions, 289_644);
}
