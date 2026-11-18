//! Round-trip tests for the ESM parser, using synthetic plugins built
//! in memory. No real game data is involved: the builders below write
//! valid plugin bytes from the UESP spec, and the tests check the
//! parser returns the right records, groups, form IDs, flags, and
//! field bytes.

use dovah_esm::{GroupChild, GroupType, Plugin, FLAG_COMPRESSED, FLAG_LIGHT_MASTER, FLAG_MASTER};
use std::io::Write;

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

/// A field whose payload is too big for a u16 size: an XXXX field
/// carrying the real u32 size, then the real field with a 0 size.
fn oversized_field(field_type: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"XXXX");
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(field_type);
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(data);
    out
}

fn record(record_type: &[u8; 4], flags: u32, form_id: u32, version: u16, fields: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(record_type);
    out.extend_from_slice(&(fields.len() as u32).to_le_bytes());
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&form_id.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // timestamp
    out.extend_from_slice(&0u16.to_le_bytes()); // version control
    out.extend_from_slice(&version.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // unknown
    out.extend_from_slice(fields);
    out
}

fn compressed_record(record_type: &[u8; 4], form_id: u32, version: u16, fields: &[u8]) -> Vec<u8> {
    use flate2::write::ZlibEncoder;
    use flate2::Compression;
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(fields).unwrap();
    let compressed = encoder.finish().unwrap();
    let mut data = Vec::new();
    data.extend_from_slice(&(fields.len() as u32).to_le_bytes());
    data.extend_from_slice(&compressed);
    record(record_type, FLAG_COMPRESSED, form_id, version, &data)
}

fn group(label: &[u8; 4], group_type: i32, children: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"GRUP");
    // Group size includes the 24-byte header.
    out.extend_from_slice(&(24u32 + children.len() as u32).to_le_bytes());
    out.extend_from_slice(label);
    out.extend_from_slice(&group_type.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // timestamp
    out.extend_from_slice(&0u16.to_le_bytes()); // version control
    out.extend_from_slice(&0u32.to_le_bytes()); // unknown
    out.extend_from_slice(children);
    out
}

fn tes4_header() -> Vec<u8> {
    let mut fields = Vec::new();
    // HEDR: version 1.7, 3 records/groups, next object ID 0x1234.
    let mut hedr = Vec::new();
    hedr.extend_from_slice(&1.7f32.to_le_bytes());
    hedr.extend_from_slice(&3u32.to_le_bytes());
    hedr.extend_from_slice(&0x1234u32.to_le_bytes());
    fields.extend_from_slice(&field(b"HEDR", &hedr));
    fields.extend_from_slice(&zstring_field(b"CNAM", "Ultron"));
    fields.extend_from_slice(&zstring_field(b"MAST", "Skyrim.esm"));
    fields.extend_from_slice(&field(b"DATA", &0u64.to_le_bytes()));
    fields.extend_from_slice(&field(b"INTV", &0u32.to_le_bytes()));
    record(b"TES4", FLAG_MASTER, 0, 44, &fields)
}

fn gmst_record(form_id: u32, edid: &str, value: &[u8]) -> Vec<u8> {
    let mut fields = Vec::new();
    fields.extend_from_slice(&zstring_field(b"EDID", edid));
    fields.extend_from_slice(&field(b"DATA", value));
    record(b"GMST", 0, form_id, 44, &fields)
}

#[test]
fn parses_header_and_top_group() {
    let mut bytes = tes4_header();
    let mut children = Vec::new();
    children.extend_from_slice(&gmst_record(0x0000_0001, "fTestOne", b"1.5"));
    children.extend_from_slice(&gmst_record(0x0000_0002, "fTestTwo", b"2.5"));
    bytes.extend_from_slice(&group(b"GMST", 0, &children));

    let plugin = Plugin::from_bytes(&bytes).unwrap();

    // Header.
    assert_eq!(plugin.header.type_str(), "TES4");
    assert_eq!(plugin.header.form_id, 0);
    assert_eq!(plugin.header.flags & FLAG_MASTER, FLAG_MASTER);
    assert_eq!(plugin.header.version, 44);
    let info = plugin.header_info().unwrap();
    assert!((info.version - 1.7).abs() < f32::EPSILON);
    assert_eq!(info.record_count, 3);
    assert_eq!(info.next_object_id, 0x1234);
    assert_eq!(plugin.masters(), vec!["Skyrim.esm"]);
    assert_eq!(plugin.header.field(b"CNAM").unwrap().as_str(), "Ultron");

    // One top group, labelled GMST.
    assert_eq!(plugin.groups.len(), 1);
    let group = &plugin.groups[0];
    assert_eq!(group.group_type, GroupType::Top);
    assert_eq!(group.label_type().as_deref(), Some("GMST"));
    assert_eq!(group.children.len(), 2);

    // The two GMST records, in order.
    let records: Vec<_> = group.records().collect();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].type_str(), "GMST");
    assert_eq!(records[0].form_id, 0x0000_0001);
    assert_eq!(records[0].edid().as_deref(), Some("fTestOne"));
    assert_eq!(records[0].field(b"DATA").unwrap().data, b"1.5".to_vec());
    assert_eq!(records[1].form_id, 0x0000_0002);
    assert_eq!(records[1].edid().as_deref(), Some("fTestTwo"));

    // Flattened view + lookup.
    assert_eq!(plugin.all_records().len(), 2);
    assert_eq!(
        plugin.find(0x0000_0002).unwrap().edid().as_deref(),
        Some("fTestTwo")
    );
    assert!(plugin.find(0x0000_9999).is_none());
}

#[test]
fn parses_nested_groups() {
    let mut bytes = tes4_header();
    // WRLD record, then a World Children subgroup holding a CELL.
    let mut world_children = Vec::new();
    let mut cell_fields = Vec::new();
    cell_fields.extend_from_slice(&zstring_field(b"EDID", "TestCell"));
    world_children.extend_from_slice(&record(b"CELL", 0, 0x0000_0010, 44, &cell_fields));
    let mut top_children = Vec::new();
    let mut wrld_fields = Vec::new();
    wrld_fields.extend_from_slice(&zstring_field(b"EDID", "TestWorld"));
    top_children.extend_from_slice(&record(b"WRLD", 0, 0x0000_0009, 44, &wrld_fields));
    top_children.extend_from_slice(&group(&0x0000_0009u32.to_le_bytes(), 1, &world_children));
    bytes.extend_from_slice(&group(b"WRLD", 0, &top_children));

    let plugin = Plugin::from_bytes(&bytes).unwrap();
    assert_eq!(plugin.groups.len(), 1);
    let top = &plugin.groups[0];
    assert_eq!(top.label_type().as_deref(), Some("WRLD"));
    assert_eq!(top.children.len(), 2);

    match &top.children[0] {
        GroupChild::Record(wrld) => {
            assert_eq!(wrld.type_str(), "WRLD");
            assert_eq!(wrld.form_id, 0x0000_0009);
        }
        other => panic!("expected WRLD record, got {other:?}"),
    }
    match &top.children[1] {
        GroupChild::Group(subgroup) => {
            assert_eq!(subgroup.group_type, GroupType::WorldChildren);
            assert_eq!(subgroup.label_form_id(), Some(0x0000_0009));
            let cells: Vec<_> = subgroup.records().collect();
            assert_eq!(cells.len(), 1);
            assert_eq!(cells[0].type_str(), "CELL");
            assert_eq!(cells[0].edid().as_deref(), Some("TestCell"));
        }
        other => panic!("expected subgroup, got {other:?}"),
    }

    // Flattened view sees both records.
    assert_eq!(plugin.all_records().len(), 2);
}

#[test]
fn parses_compressed_record() {
    let mut bytes = tes4_header();
    let mut fields = Vec::new();
    fields.extend_from_slice(&zstring_field(b"EDID", "CompressedSetting"));
    fields.extend_from_slice(&field(b"DATA", b"packed payload"));
    let children = compressed_record(b"GMST", 0x0000_0003, 44, &fields);
    bytes.extend_from_slice(&group(b"GMST", 0, &children));

    let plugin = Plugin::from_bytes(&bytes).unwrap();
    let record = plugin.find(0x0000_0003).unwrap();
    assert!(record.was_compressed());
    assert_eq!(record.edid().as_deref(), Some("CompressedSetting"));
    assert_eq!(
        record.field(b"DATA").unwrap().data,
        b"packed payload".to_vec()
    );
}

#[test]
fn parses_xxxx_oversized_field() {
    let mut bytes = tes4_header();
    // 70,000-byte payload: too big for a u16 field size.
    let big: Vec<u8> = (0..70_000u32).map(|i| (i % 251) as u8).collect();
    let mut fields = Vec::new();
    fields.extend_from_slice(&zstring_field(b"EDID", "BigNavmesh"));
    fields.extend_from_slice(&oversized_field(b"NVMI", &big));
    let children = record(b"NAVM", 0, 0x0000_0004, 44, &fields);
    bytes.extend_from_slice(&group(b"NAVM", 0, &children));

    let plugin = Plugin::from_bytes(&bytes).unwrap();
    let record = plugin.find(0x0000_0004).unwrap();
    assert_eq!(record.field(b"NVMI").unwrap().data, big);
    // XXXX itself is not kept as a field.
    assert!(record.field(b"XXXX").is_none());
}

#[test]
fn rejects_non_tes4_first_record() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&gmst_record(0x0000_0001, "fNope", b"1"));
    assert!(Plugin::from_bytes(&bytes).is_err());
}

#[test]
fn rejects_truncated_and_empty_input() {
    assert!(Plugin::from_bytes(&[]).is_err());
    let mut bytes = tes4_header();
    bytes.truncate(10);
    assert!(Plugin::from_bytes(&bytes).is_err());

    // Truncated record data inside a group.
    let mut full = tes4_header();
    let children = gmst_record(0x0000_0001, "fCut", b"1234");
    full.extend_from_slice(&group(b"GMST", 0, &children));
    full.truncate(full.len() - 2);
    assert!(Plugin::from_bytes(&full).is_err());
}

#[test]
fn record_flags_are_preserved() {
    let mut bytes = tes4_header();
    let flags = FLAG_LIGHT_MASTER | 0x0000_0800; // light master + initially disabled
    let children = record(b"GMST", flags, 0x0000_0005, 44, &[]);
    bytes.extend_from_slice(&group(b"GMST", 0, &children));
    let plugin = Plugin::from_bytes(&bytes).unwrap();
    let record = plugin.find(0x0000_0005).unwrap();
    assert_eq!(record.flags, flags);
}
