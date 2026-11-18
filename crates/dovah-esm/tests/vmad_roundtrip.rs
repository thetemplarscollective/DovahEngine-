//! Synthetic tests for the VMAD parser: hand-built VMAD payloads
//! following the UESP spec, covering every property type, both
//! object formats, old versions, and every fragments layout. No
//! real game data is involved; the real-data check lives in
//! `vmad_real.rs`.

use dovah_esm::vmad::{
    Fragment, Fragments, IndexedFragment, PhaseFragment, QuestAlias, QuestFragment,
};
use dovah_esm::{Vmad, VmadValue};

fn wstring(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(&(s.len() as u16).to_le_bytes());
    out.extend_from_slice(s.as_bytes());
}

/// An object reference in objFormat 2 layout: unused u16, alias u16,
/// form ID u32.
fn object_v2(out: &mut Vec<u8>, form_id: u32, alias: u16) {
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&alias.to_le_bytes());
    out.extend_from_slice(&form_id.to_le_bytes());
}

/// An object reference in objFormat 1 layout: form ID u32, alias
/// u16, unused u16.
fn object_v1(out: &mut Vec<u8>, form_id: u32, alias: u16) {
    out.extend_from_slice(&form_id.to_le_bytes());
    out.extend_from_slice(&alias.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
}

fn header(out: &mut Vec<u8>, version: i16, obj_format: i16, script_count: u16) {
    out.extend_from_slice(&version.to_le_bytes());
    out.extend_from_slice(&obj_format.to_le_bytes());
    out.extend_from_slice(&script_count.to_le_bytes());
}

fn script_header(out: &mut Vec<u8>, name: &str, version: i16, property_count: u16) {
    wstring(out, name);
    if version >= 4 {
        out.push(0); // status: local script
    }
    out.extend_from_slice(&property_count.to_le_bytes());
}

fn property_header(out: &mut Vec<u8>, name: &str, property_type: u8, version: i16) {
    wstring(out, name);
    out.push(property_type);
    if version >= 4 {
        out.push(1); // status: edited
    }
}

#[test]
fn all_property_types_v5_objformat2() {
    let mut bytes = Vec::new();
    header(&mut bytes, 5, 2, 1);
    script_header(&mut bytes, "TestScript", 5, 10);

    property_header(&mut bytes, "AnObject", 1, 5);
    object_v2(&mut bytes, 0x0001_2345, 0xFFFF);
    property_header(&mut bytes, "AnAlias", 1, 5);
    object_v2(&mut bytes, 0x0000_ABCD, 7);
    property_header(&mut bytes, "AString", 2, 5);
    wstring(&mut bytes, "hello");
    property_header(&mut bytes, "AnInt", 3, 5);
    bytes.extend_from_slice(&(-42i32).to_le_bytes());
    property_header(&mut bytes, "AFloat", 4, 5);
    bytes.extend_from_slice(&2.5f32.to_le_bytes());
    property_header(&mut bytes, "ABool", 5, 5);
    bytes.push(1);
    property_header(&mut bytes, "IntArray", 13, 5);
    bytes.extend_from_slice(&3u32.to_le_bytes());
    for v in [1i32, 2, 3] {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    property_header(&mut bytes, "StrArray", 12, 5);
    bytes.extend_from_slice(&2u32.to_le_bytes());
    wstring(&mut bytes, "a");
    wstring(&mut bytes, "b");
    property_header(&mut bytes, "ObjArray", 11, 5);
    bytes.extend_from_slice(&1u32.to_le_bytes());
    object_v2(&mut bytes, 0x0000_9999, 0xFFFF);
    property_header(&mut bytes, "EmptyFloatArray", 14, 5);
    bytes.extend_from_slice(&0u32.to_le_bytes());

    let vmad = Vmad::parse(&bytes, b"WEAP").unwrap();
    assert_eq!(vmad.version, 5);
    assert_eq!(vmad.obj_format, 2);
    assert_eq!(vmad.scripts.len(), 1);
    assert!(vmad.fragments.is_none());
    let script = &vmad.scripts[0];
    assert_eq!(script.name, "TestScript");
    assert_eq!(script.status, 0);
    let props = &script.properties;
    assert_eq!(props.len(), 10);
    assert_eq!(props[0].name, "AnObject");
    assert_eq!(
        props[0].value,
        VmadValue::Object {
            form_id: 0x0001_2345,
            alias_id: None
        }
    );
    assert_eq!(
        props[1].value,
        VmadValue::Object {
            form_id: 0x0000_ABCD,
            alias_id: Some(7)
        }
    );
    assert_eq!(props[2].value, VmadValue::Str("hello".to_string()));
    assert_eq!(props[3].value, VmadValue::Int(-42));
    assert_eq!(props[4].value, VmadValue::Float(2.5));
    assert_eq!(props[5].value, VmadValue::Bool(true));
    assert_eq!(
        props[6].value,
        VmadValue::Array(vec![
            VmadValue::Int(1),
            VmadValue::Int(2),
            VmadValue::Int(3),
        ])
    );
    assert_eq!(
        props[7].value,
        VmadValue::Array(vec![
            VmadValue::Str("a".to_string()),
            VmadValue::Str("b".to_string()),
        ])
    );
    assert_eq!(
        props[8].value,
        VmadValue::Array(vec![VmadValue::Object {
            form_id: 0x0000_9999,
            alias_id: None,
        }])
    );
    assert_eq!(props[9].value, VmadValue::Array(vec![]));
    // Every property status was stored as edited.
    assert!(props.iter().all(|p| p.status == 1));
}

#[test]
fn objformat1_object_layout() {
    let mut bytes = Vec::new();
    header(&mut bytes, 5, 1, 1);
    script_header(&mut bytes, "S", 5, 1);
    property_header(&mut bytes, "P", 1, 5);
    object_v1(&mut bytes, 0x00AA_BBCC, 3);

    let vmad = Vmad::parse(&bytes, b"WEAP").unwrap();
    assert_eq!(
        vmad.scripts[0].properties[0].value,
        VmadValue::Object {
            form_id: 0x00AA_BBCC,
            alias_id: Some(3)
        }
    );
}

#[test]
fn version2_has_no_status_bytes() {
    let mut bytes = Vec::new();
    header(&mut bytes, 2, 2, 1);
    script_header(&mut bytes, "OldScript", 2, 1);
    property_header(&mut bytes, "P", 3, 2);
    bytes.extend_from_slice(&7i32.to_le_bytes());

    let vmad = Vmad::parse(&bytes, b"WEAP").unwrap();
    // Status bytes are absent before version 4: script defaults to
    // 0 (local), property defaults to 1 (edited).
    assert_eq!(vmad.scripts[0].status, 0);
    assert_eq!(vmad.scripts[0].properties[0].status, 1);
    assert_eq!(vmad.scripts[0].properties[0].value, VmadValue::Int(7));
}

#[test]
fn version4_has_status_bytes() {
    let mut bytes = Vec::new();
    header(&mut bytes, 4, 2, 1);
    wstring(&mut bytes, "S");
    bytes.push(1); // inherited with altered properties
    bytes.extend_from_slice(&1u16.to_le_bytes());
    wstring(&mut bytes, "P");
    bytes.push(3);
    bytes.push(3); // property removed
    bytes.extend_from_slice(&9i32.to_le_bytes());

    let vmad = Vmad::parse(&bytes, b"WEAP").unwrap();
    assert_eq!(vmad.scripts[0].status, 1);
    assert_eq!(vmad.scripts[0].properties[0].status, 3);
}

#[test]
fn zero_scripts_parses() {
    let mut bytes = Vec::new();
    header(&mut bytes, 5, 2, 0);
    let vmad = Vmad::parse(&bytes, b"WEAP").unwrap();
    assert!(vmad.scripts.is_empty());
    assert!(vmad.fragments.is_none());
}

#[test]
fn info_fragments() {
    let mut bytes = Vec::new();
    header(&mut bytes, 5, 2, 0);
    bytes.push(2); // unknown, always 2
    bytes.push(0x03); // begin + end
    wstring(&mut bytes, "TIF__00012345");
    // Begin fragment.
    bytes.push(0);
    wstring(&mut bytes, "TIF__00012345");
    wstring(&mut bytes, "Fragment_0");
    // End fragment.
    bytes.push(0);
    wstring(&mut bytes, "TIF__00012345");
    wstring(&mut bytes, "Fragment_1");

    let vmad = Vmad::parse(&bytes, b"INFO").unwrap();
    assert_eq!(
        vmad.fragments,
        Some(Fragments::Flagged {
            file_name: "TIF__00012345".to_string(),
            flags: 0x03,
            fragments: vec![
                Fragment {
                    script_name: "TIF__00012345".to_string(),
                    fragment_name: "Fragment_0".to_string(),
                },
                Fragment {
                    script_name: "TIF__00012345".to_string(),
                    fragment_name: "Fragment_1".to_string(),
                },
            ],
        })
    );
}

#[test]
fn perk_fragments() {
    let mut bytes = Vec::new();
    header(&mut bytes, 5, 2, 0);
    bytes.push(2);
    wstring(&mut bytes, "PRKF_Test_0001");
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&3u16.to_le_bytes()); // index
    bytes.extend_from_slice(&0i16.to_le_bytes());
    bytes.push(0);
    wstring(&mut bytes, "PRKF_Test_0001");
    wstring(&mut bytes, "Fragment_3");

    let vmad = Vmad::parse(&bytes, b"PERK").unwrap();
    assert_eq!(
        vmad.fragments,
        Some(Fragments::Perk {
            file_name: "PRKF_Test_0001".to_string(),
            fragments: vec![IndexedFragment {
                index: 3,
                script_name: "PRKF_Test_0001".to_string(),
                fragment_name: "Fragment_3".to_string(),
            }],
        })
    );
}

#[test]
fn quest_fragments_and_aliases() {
    let mut bytes = Vec::new();
    header(&mut bytes, 5, 2, 1);
    script_header(&mut bytes, "QuestScript", 5, 0);
    bytes.push(2);
    bytes.extend_from_slice(&1u16.to_le_bytes()); // fragment count
    wstring(&mut bytes, "QF_TestQuest_0001");
    // One stage fragment: stage 10, log entry -1.
    bytes.extend_from_slice(&10u16.to_le_bytes());
    bytes.extend_from_slice(&0i16.to_le_bytes());
    bytes.extend_from_slice(&(-1i32).to_le_bytes());
    bytes.push(1);
    wstring(&mut bytes, "QF_TestQuest_0001");
    wstring(&mut bytes, "Fragment_10");
    // One alias with one script.
    bytes.extend_from_slice(&1u16.to_le_bytes()); // alias count
    object_v2(&mut bytes, 0x0000_1111, 5);
    bytes.extend_from_slice(&5i16.to_le_bytes()); // alias version
    bytes.extend_from_slice(&2i16.to_le_bytes()); // alias objFormat
    bytes.extend_from_slice(&1u16.to_le_bytes()); // alias script count
    script_header(&mut bytes, "AliasScript", 5, 1);
    property_header(&mut bytes, "P", 5, 5);
    bytes.push(0);

    let vmad = Vmad::parse(&bytes, b"QUST").unwrap();
    assert_eq!(
        vmad.fragments,
        Some(Fragments::Quest {
            file_name: "QF_TestQuest_0001".to_string(),
            fragments: vec![QuestFragment {
                stage_index: 10,
                log_entry: -1,
                script_name: "QF_TestQuest_0001".to_string(),
                fragment_name: "Fragment_10".to_string(),
            }],
            aliases: vec![QuestAlias {
                form_id: 0x0000_1111,
                alias_id: Some(5),
                scripts: vec![dovah_esm::VmadScript {
                    name: "AliasScript".to_string(),
                    status: 0,
                    properties: vec![dovah_esm::VmadProperty {
                        name: "P".to_string(),
                        status: 1,
                        value: VmadValue::Bool(false),
                    }],
                }],
            }],
        })
    );
}

#[test]
fn scene_fragments_and_phases() {
    let mut bytes = Vec::new();
    header(&mut bytes, 5, 2, 0);
    bytes.push(2);
    bytes.push(0x01); // begin only
    wstring(&mut bytes, "SF_Test_0001");
    bytes.push(0);
    wstring(&mut bytes, "SF_Test_0001");
    wstring(&mut bytes, "Fragment_0");
    bytes.extend_from_slice(&1u16.to_le_bytes()); // phase count
    bytes.push(0);
    bytes.extend_from_slice(&2u32.to_le_bytes()); // phase 2
    bytes.push(0);
    wstring(&mut bytes, "SF_Test_0001");
    wstring(&mut bytes, "Fragment_Phase_2");

    let vmad = Vmad::parse(&bytes, b"SCEN").unwrap();
    assert_eq!(
        vmad.fragments,
        Some(Fragments::Scene {
            file_name: "SF_Test_0001".to_string(),
            flags: 0x01,
            begin_end: vec![Fragment {
                script_name: "SF_Test_0001".to_string(),
                fragment_name: "Fragment_0".to_string(),
            }],
            phases: vec![PhaseFragment {
                phase: 2,
                script_name: "SF_Test_0001".to_string(),
                fragment_name: "Fragment_Phase_2".to_string(),
            }],
        })
    );
}

#[test]
fn errors_on_bad_version_unknown_type_and_trailing_bytes() {
    // Bad version.
    let mut bytes = Vec::new();
    header(&mut bytes, 6, 2, 0);
    assert!(Vmad::parse(&bytes, b"WEAP").is_err());

    // Bad object format.
    let mut bytes = Vec::new();
    header(&mut bytes, 5, 3, 0);
    assert!(Vmad::parse(&bytes, b"WEAP").is_err());

    // Unknown property type.
    let mut bytes = Vec::new();
    header(&mut bytes, 5, 2, 1);
    script_header(&mut bytes, "S", 5, 1);
    property_header(&mut bytes, "P", 9, 5);
    assert!(Vmad::parse(&bytes, b"WEAP").is_err());

    // Array property before version 5.
    let mut bytes = Vec::new();
    header(&mut bytes, 4, 2, 1);
    script_header(&mut bytes, "S", 4, 1);
    property_header(&mut bytes, "P", 13, 4);
    assert!(Vmad::parse(&bytes, b"WEAP").is_err());

    // Trailing bytes on a record type that never has fragments.
    let mut bytes = Vec::new();
    header(&mut bytes, 5, 2, 0);
    bytes.push(2);
    assert!(Vmad::parse(&bytes, b"WEAP").is_err());

    // Truncated: header cut short.
    assert!(Vmad::parse(&[5, 0], b"WEAP").is_err());
    // Truncated: script name length runs past the end.
    let mut bytes = Vec::new();
    header(&mut bytes, 5, 2, 1);
    bytes.extend_from_slice(&100u16.to_le_bytes());
    bytes.extend_from_slice(b"short");
    assert!(Vmad::parse(&bytes, b"WEAP").is_err());
}

#[test]
fn record_vmad_convenience() {
    // Build a tiny plugin with one WEAP record carrying a VMAD field,
    // and check Record::vmad() parses it while a record without VMAD
    // returns None.
    fn field(field_type: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(field_type);
        out.extend_from_slice(&(data.len() as u16).to_le_bytes());
        out.extend_from_slice(data);
        out
    }
    fn record(record_type: &[u8; 4], form_id: u32, fields: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(record_type);
        out.extend_from_slice(&(fields.len() as u32).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes()); // flags
        out.extend_from_slice(&form_id.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // timestamp
        out.extend_from_slice(&0u16.to_le_bytes()); // version control
        out.extend_from_slice(&44u16.to_le_bytes()); // version
        out.extend_from_slice(&0u16.to_le_bytes()); // unknown
        out.extend_from_slice(fields);
        out
    }
    fn group(label: &[u8; 4], children: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"GRUP");
        out.extend_from_slice(&(24 + children.len() as u32).to_le_bytes());
        out.extend_from_slice(label);
        out.extend_from_slice(&0i32.to_le_bytes()); // top group
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(children);
        out
    }

    let mut vmad_bytes = Vec::new();
    header(&mut vmad_bytes, 5, 2, 1);
    script_header(&mut vmad_bytes, "SwordScript", 5, 0);

    let mut tes4_fields = Vec::new();
    tes4_fields.extend_from_slice(&field(b"HEDR", &{
        let mut h = Vec::new();
        h.extend_from_slice(&1.7f32.to_le_bytes());
        h.extend_from_slice(&2u32.to_le_bytes());
        h.extend_from_slice(&0u32.to_le_bytes());
        h
    }));
    let mut weap_fields = Vec::new();
    weap_fields.extend_from_slice(&field(b"EDID", b"IronSword\0"));
    weap_fields.extend_from_slice(&field(b"VMAD", &vmad_bytes));
    let mut misc_fields = Vec::new();
    misc_fields.extend_from_slice(&field(b"EDID", b"PlainItem\0"));

    let mut plugin_bytes = Vec::new();
    plugin_bytes.extend_from_slice(&record(b"TES4", 0, &tes4_fields));
    let mut weap_children = Vec::new();
    weap_children.extend_from_slice(&record(b"WEAP", 0x1234, &weap_fields));
    plugin_bytes.extend_from_slice(&group(b"WEAP", &weap_children));
    let mut misc_children = Vec::new();
    misc_children.extend_from_slice(&record(b"MISC", 0x5678, &misc_fields));
    plugin_bytes.extend_from_slice(&group(b"MISC", &misc_children));

    let plugin = dovah_esm::Plugin::from_bytes(&plugin_bytes).unwrap();
    let records = plugin.all_records();
    assert_eq!(records.len(), 2);
    let sword = records.iter().find(|r| r.form_id == 0x1234).unwrap();
    let parsed = sword.vmad().expect("WEAP should have VMAD").unwrap();
    assert_eq!(parsed.scripts[0].name, "SwordScript");
    let plain = records.iter().find(|r| r.form_id == 0x5678).unwrap();
    assert!(plain.vmad().is_none());
}
