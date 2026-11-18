//! Real-data check for the strings reader: read Dylan's real
//! `Skyrim_English.STRINGS` / `.ILSTRINGS` (extracted from Nexus
//! mod 1842, identical to the files inside `Skyrim_-_Patch.bsa`)
//! and verify known IDs resolve to their exact text. Skips
//! (passes) when the files are not present — the synthetic tests
//! in `lib.rs` are the ones that run everywhere.
//!
//! The pinned strings come from the `dialogue_probe3` example
//! (see `crates/dovah-app/examples/dialogue_probe3.rs`): NAM1
//! 62071 is Delphine's first response under her Thalmor topic,
//! and the FULL ids are the inn NPCs' names and the Iron Sword.

use dovah_strings::{StringsFile, StringsKind};
use std::path::PathBuf;

fn strings_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("test-data")
        .join("strings")
        .join("Data")
        .join("Strings")
}

#[test]
fn known_strings_resolve_to_exact_text() {
    let dir = strings_dir();
    let general_path = dir.join("Skyrim_English.STRINGS");
    let dialogue_path = dir.join("Skyrim_English.ILSTRINGS");
    if !general_path.exists() || !dialogue_path.exists() {
        eprintln!(
            "skipping real-data strings check: {} not present",
            dir.display()
        );
        return;
    }
    let general_bytes = std::fs::read(&general_path).expect("STRINGS should read");
    let dialogue_bytes = std::fs::read(&dialogue_path).expect("ILSTRINGS should read");
    let general = StringsFile::from_bytes(&general_bytes, StringsKind::General)
        .expect("STRINGS should parse");
    let dialogue = StringsFile::from_bytes(&dialogue_bytes, StringsKind::LengthPrefixed)
        .expect("ILSTRINGS should parse");

    // Header counts (Nexus 1842 = Patch.bsa, verified).
    assert_eq!(general.len(), 30_301, "STRINGS entries");
    assert_eq!(dialogue.len(), 34_427, "ILSTRINGS entries");

    // FULL ids -> .STRINGS (names).
    assert_eq!(general.get(378), Some("Delphine"));
    assert_eq!(general.get(56228), Some("Orgnar"));
    assert_eq!(general.get(60445), Some("Embry"));
    assert_eq!(general.get(1763), Some("Iron Sword"));

    // INFO NAM1 ids -> .ILSTRINGS (dialogue lines), exact text.
    assert_eq!(
        dialogue.get(62071),
        Some("If we could get into the Thalmor Embassy... it's the centre of their operations in Skyrim.")
    );
    assert_eq!(
        dialogue.get(63455),
        Some("I'm not sure yet. I have a few ideas, but I'll need some time to pull things together....")
    );

    // The key M18 correction: NAM1 ids do NOT live in
    // .STRINGS — a reader looking in the wrong file finds
    // nothing (or the wrong string), never the line.
    assert_ne!(
        general.get(62071),
        Some("If we could get into the Thalmor Embassy... it's the centre of their operations in Skyrim.")
    );
}
