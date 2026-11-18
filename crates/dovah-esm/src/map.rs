//! Map-marker type names + world-map marker cells for the
//! character menu's Map tab (UI milestone 2).
//!
//! Every fact here is proven from Dylan's real `Skyrim.esm` by
//! the `m2_probe2` / `m2_probe3` / `m2_probe4` examples in
//! dovah-app (kept as the durable research record).
//!
//! ## Marker records
//!
//! A map marker is a REFR carrying an **empty XMRK field** —
//! presence is the marker, there is no type byte in XMRK. The
//! type is **TNAM (u16)**; FNAM (1 byte) is a flags byte whose
//! semantics are UNVERIFIED (values 0/1/3 observed, not used
//! for display); XLRT points at an LCRT record (usually
//! MapMarkerRefType 0x0010f63c), not the icon type. FULL
//! resolves in `.STRINGS` for all 397 XMRK refs (0 misses).
//!
//! ## Census (world map: 358 markers)
//!
//! * 347 markers in Tamriel, all children of the persistent
//!   cell 0x00000d74.
//! * 11 markers in the five city worldspaces, with positions in
//!   the TAMRIEL frame (verified geographically — Whiterun
//!   (19855,-7422) sits north of Riverwood (21941,-44792)):
//!   the five city markers (Whiterun 40, Riften 36, Windhelm 38,
//!   Markarth 44, Solitude 42 — EDID `SolitudeMapmarkerRef`,
//!   lowercase 'm') plus four palace markers (Dragonsreach 39,
//!   Palace of the Kings 37, Blue Palace 41, Understone Keep
//!   43) plus two Riften markers the EDID-driven probe missed
//!   (Riften's persistent cell holds 3 XMRK refs, not 1):
//!   Mistveil Keep (35) and the Thieves Guild (11, `TGMapMarker`).
//!   College of Winterhold (45) and Winterhold (46) are
//!   already in the Tamriel set.
//! * The remaining ~41 XMRK refs live in interior cells
//!   (local-map / quest context) and are not plotted.
//!
//! ## TNAM mapping
//!
//! Derived from named samples + the marker sprite names in
//! `interface/map.swf` (which defines the icon set). The tricky
//! distinctions are sample-proven: TNAM=5 camps are bandit camps
//! (Karthspire, Robber's Gorge) while TNAM=29 camps are GIANT
//! camps (Broken Limb, Talking Stone); TNAM=30 is shacks
//! (Meeko's Shack, Hunter's Rest), not ruins; TNAM=32 is Nordic
//! dwellings (Journeyman's Nook, Skytemple Ruins); TNAM=33 is
//! the East Empire Company Warehouse (Docks). TNAM=13 vs 19 are
//! both farms in the samples — 13 Farm / 19 Wheat Mill follows
//! the `.swf` sprite set; the in-game distinction is unverified.
//! Unobserved values (1, 20, 25, 26, 35, 47, 49, 51, 53) are
//! named from the `.swf` sprite set as best-effort and marked
//! as such in the table below.

/// Tamriel's persistent cell — all 347 Tamriel map markers are
/// its children. (cell.rs has the same value as a private const;
/// this is the ESM-facing copy for the map index.)
pub const TAMRIEL_PERSISTENT_CELL: u32 = 0x0000_0d74;

/// The persistent cell of each city worldspace — the city map
/// markers are children of these cells (like Tamriel's markers
/// under 0x00000d74). Form IDs proven by probe 3/4.
pub const CITY_MARKER_CELLS: [u32; 5] = [
    0x0001_a270, // WhiterunWorld persistent cell
    0x0001_6be0, // RiftenWorld persistent cell
    0x0001_691f, // WindhelmWorld persistent cell
    0x0001_6e02, // MarkarthWorld persistent cell
    0x0003_7ee6, // SolitudeWorld persistent cell
];

/// The display name for a map-marker TNAM type value. Unknown
/// values read "Location" — the name is still shown from the
/// marker's FULL, so an unmapped type never hides a place.
pub fn marker_type_name(tnam: u16) -> &'static str {
    match tnam {
        0 => "Location",
        1 => "City", // unobserved in Skyrim.esm
        2 => "Town",
        3 => "Settlement",
        4 => "Cave",
        5 => "Camp",
        6 => "Fort",
        7 => "Nordic Ruin",
        8 => "Dwemer Ruin",
        9 => "Shipwreck",
        10 => "Grove",
        11 => "Landmark",
        12 => "Dragon Lair",
        13 => "Farm",
        14 => "Wood Mill",
        15 => "Mine",
        16 => "Imperial Camp",
        17 => "Stormcloak Camp",
        18 => "Doomstone",
        19 => "Wheat Mill",
        20 => "Smelter", // unobserved in Skyrim.esm
        21 => "Stable",
        22 => "Imperial Tower",
        23 => "Clearing",
        24 => "Pass",
        25 => "Altar", // unobserved in Skyrim.esm
        26 => "Rock",  // unobserved in Skyrim.esm
        27 => "Lighthouse",
        28 => "Orc Stronghold",
        29 => "Giant Camp",
        30 => "Shack",
        31 => "Nordic Tower",
        32 => "Nordic Dwelling",
        33 => "Docks",
        34 => "Shrine",
        35 => "Mistveil Keep", // Riften's keep (0x000c44b1) — like 37/39/41/43, a per-palace TNAM
        36 => "Riften",
        37 => "Palace of the Kings",
        38 => "Windhelm",
        39 => "Dragonsreach",
        40 => "Whiterun",
        41 => "Blue Palace",
        42 => "Solitude",
        43 => "Understone Keep",
        44 => "Markarth",
        45 => "College of Winterhold",
        46 => "Winterhold",
        47 => "Location", // unobserved in Skyrim.esm
        48 => "Morthal",
        49 => "Location", // unobserved in Skyrim.esm
        50 => "Falkreath",
        51 => "Location", // unobserved in Skyrim.esm
        52 => "Dawnstar",
        53 => "Location", // unobserved in Skyrim.esm
        _ => "Location",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_type_names_match_the_samples() {
        // Sample-proven pairs from probe 2.
        assert_eq!(marker_type_name(2), "Town"); // Riverwood
        assert_eq!(marker_type_name(4), "Cave"); // Sunderstone Gorge
        assert_eq!(marker_type_name(5), "Camp"); // Karthspire Camp
        assert_eq!(marker_type_name(6), "Fort"); // Fort Hraggstad
        assert_eq!(marker_type_name(7), "Nordic Ruin"); // Bleak Falls Barrow
        assert_eq!(marker_type_name(8), "Dwemer Ruin"); // Great Lift at Alftand
        assert_eq!(marker_type_name(15), "Mine"); // Embershard Mine
        assert_eq!(marker_type_name(18), "Doomstone"); // The Guardian Stones
        assert_eq!(marker_type_name(27), "Lighthouse"); // Solitude Lighthouse
        assert_eq!(marker_type_name(28), "Orc Stronghold"); // Narzulbur
        assert_eq!(marker_type_name(29), "Giant Camp"); // Broken Limb Camp
        assert_eq!(marker_type_name(30), "Shack"); // Meeko's Shack
        assert_eq!(marker_type_name(31), "Nordic Tower"); // Bannermist Tower
        assert_eq!(marker_type_name(32), "Nordic Dwelling"); // Journeyman's Nook
        assert_eq!(marker_type_name(33), "Docks"); // East Empire Company Warehouse
        assert_eq!(marker_type_name(34), "Shrine"); // Shrine of Azura
        assert_eq!(marker_type_name(35), "Mistveil Keep"); // Riften's keep (marker_count_probe)
        assert_eq!(marker_type_name(40), "Whiterun");
        assert_eq!(marker_type_name(42), "Solitude");
        assert_eq!(marker_type_name(45), "College of Winterhold");
        assert_eq!(marker_type_name(99), "Location");
    }

    #[test]
    fn city_marker_cells_are_the_five_city_worlds() {
        assert_eq!(CITY_MARKER_CELLS.len(), 5);
        assert!(CITY_MARKER_CELLS.contains(&0x0001_a270)); // Whiterun
        assert!(CITY_MARKER_CELLS.contains(&0x0003_7ee6)); // Solitude
    }
}
