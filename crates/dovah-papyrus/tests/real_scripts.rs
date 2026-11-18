//! Real-script proving tests (M7.1): load Dylan's actual .pex
//! files and run them. Skips (passes) when the scripts directory
//! is not present — the synthetic tests in `papyrus_runtime.rs`
//! are the ones that run everywhere.
//!
//! The pinned values come from reading the real disassembly (see
//! `crates/dovah-papyrus/examples/prove_real_scripts.rs`); they
//! are exact values from the real scripts, not estimates.

use dovah_papyrus::error::RuntimeError;
use dovah_papyrus::resolve::resolve_function;
use dovah_papyrus::value::RuntimeValue;
use dovah_papyrus::Vm;
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

fn present() -> bool {
    scripts_dir().join("cwscript.pex").exists()
}

/// Load one script (by lowercase file stem) into the VM.
fn load(vm: &mut Vm, stem: &str) {
    let path = scripts_dir().join(format!("{stem}.pex"));
    let bytes = fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let file = PexFile::from_bytes(&bytes).expect("parse pex");
    vm.load_pex(&file);
}

fn load_many(vm: &mut Vm, stems: &[&str]) {
    for stem in stems {
        load(vm, stem);
    }
}

/// Call a function on an instance and return the value it
/// returned, asserting the call ran clean (no new errors).
fn call_clean(
    vm: &mut Vm,
    id: dovah_papyrus::InstanceId,
    name: &str,
    args: Vec<RuntimeValue>,
) -> RuntimeValue {
    let errors_before = vm.errors.len();
    assert!(
        vm.call_function(id, name, args),
        "call {name} was not posted"
    );
    assert!(vm.run_until_idle(1000, 100), "call {name} did not drain");
    assert_eq!(
        vm.errors.len(),
        errors_before,
        "call {name} produced errors: {:?}",
        &vm.errors[errors_before..]
    );
    vm.last_stack_result.clone().unwrap_or(RuntimeValue::None)
}

/// CWScript.HoldName maps hold numbers to hold names, with the
/// PEX defaults iHaafingar=1 .. iRift=9 and "unknown hold" as the
/// fall-through. All nine holds plus two unknowns, from the real
/// 37-instruction function.
#[test]
fn hold_name_all_holds() {
    if !present() {
        eprintln!("skipping real-script test: scripts dir not present");
        return;
    }
    let mut vm = Vm::new();
    load_many(&mut vm, &["cwscript", "quest", "form"]);
    let cw = vm.create_instance(0x1000, "CWScript", None).unwrap();
    assert!(vm.run_until_idle(1000, 100));
    let holds = [
        (1, "Haafingar"),
        (2, "Reach"),
        (3, "Hjaalmarch"),
        (4, "Whiterun"),
        (5, "Falkreath"),
        (6, "Pale"),
        (7, "Winterhold"),
        (8, "Eastmarch"),
        (9, "Rift"),
        (0, "unknown hold"),
        (99, "unknown hold"),
    ];
    for (number, expected) in holds {
        let got = call_clean(&mut vm, cw, "HoldName", vec![RuntimeValue::Int(number)]);
        assert_eq!(
            got,
            RuntimeValue::Str(std::rc::Rc::from(expected)),
            "HoldName({number})"
        );
    }
}

/// CraftSmithing.GetMinSkill returns the smithing skill needed
/// for a material keyword: steel is 5, anything else falls
/// through to 0. The steel keyword is wired into the
/// ::WeaponMaterialSteel_var the way VMAD would wire it.
#[test]
fn get_min_skill_steel_and_fallthrough() {
    if !present() {
        eprintln!("skipping real-script test: scripts dir not present");
        return;
    }
    let mut vm = Vm::new();
    load_many(
        &mut vm,
        &["craftsmithing", "objectreference", "form", "keyword"],
    );
    let smith = vm.create_instance(0x1001, "CraftSmithing", None).unwrap();
    let steel = vm.create_instance(0x2000, "Keyword", None).unwrap();
    let other = vm.create_instance(0x2001, "Keyword", None).unwrap();
    assert!(vm.run_until_idle(1000, 100));
    vm.instances
        .get_mut(smith)
        .unwrap()
        .set("::WeaponMaterialSteel_var", RuntimeValue::Object(steel));
    let got = call_clean(
        &mut vm,
        smith,
        "GetMinSkill",
        vec![RuntimeValue::Object(steel)],
    );
    assert_eq!(got, RuntimeValue::Int(5));
    let got = call_clean(
        &mut vm,
        smith,
        "GetMinSkill",
        vec![RuntimeValue::Object(other)],
    );
    assert_eq!(got, RuntimeValue::Int(0));
}

/// defaultOnEnter.OnInit counts the non-None trigger vars into
/// targetCountTotal. With no VMAD every trigger var is None, so
/// the count is 0 — and the variable itself starts at 0 (its PEX
/// default is None, which means "no explicit default", so the
/// type's zero value applies — M7.1).
#[test]
fn default_on_enter_on_init_counts_nothing() {
    if !present() {
        eprintln!("skipping real-script test: scripts dir not present");
        return;
    }
    let mut vm = Vm::new();
    load_many(&mut vm, &["defaultonenter", "objectreference", "form"]);
    let errors_before = vm.errors.len();
    let id = vm.create_instance(0x1002, "defaultOnEnter", None).unwrap();
    assert!(vm.run_until_idle(1000, 100));
    assert_eq!(vm.errors.len(), errors_before, "OnInit produced errors");
    let total = vm
        .instances
        .get(id)
        .and_then(|i| i.get("targetCountTotal").cloned());
    assert_eq!(total, Some(RuntimeValue::Int(0)));
    let got = call_clean(&mut vm, id, "GetTotalTargetCount", vec![]);
    assert_eq!(got, RuntimeValue::Int(0));
}

/// `not` on an object tests for None and `not` on an int tests
/// for 0 (M7.1): CWFortGateTriggerScript.OnInit `not`s four
/// object references, WETriggerScript.OnInit `not`s a location.
/// Both run clean with no VMAD.
#[test]
fn not_on_object_and_int_oninit_runs_clean() {
    if !present() {
        eprintln!("skipping real-script test: scripts dir not present");
        return;
    }
    let mut vm = Vm::new();
    load_many(
        &mut vm,
        &[
            "cwfortgatetriggerscript",
            "wetriggerscript",
            "trigger",
            "objectreference",
            "form",
        ],
    );
    for (form, script) in [
        (0x1010, "CWFortGateTriggerScript"),
        (0x1011, "WETriggerScript"),
    ] {
        let errors_before = vm.errors.len();
        vm.create_instance(form, script, None).unwrap();
        assert!(
            vm.run_until_idle(1000, 100),
            "{script} OnInit did not drain"
        );
        assert_eq!(
            vm.errors.len(),
            errors_before,
            "{script} OnInit produced errors: {:?}",
            &vm.errors[errors_before..]
        );
    }
}

/// The Sleeping Giant Inn's room trigger, end to end: OnLoad
/// wires the target ref to the player, entering sets
/// playerInRoom on the MQ106 quest script — and then the chain
/// stops at the one native the engine does not have yet,
/// Quest.SetCurrentStageID (behind the script wrapper
/// Quest.SetStage). Leaving sets playerInRoom back to false,
/// and then stops on Delphine's unwired alias (no VMAD here).
/// When SetCurrentStageID lands, this test must be updated to
/// assert the full clean run.
#[test]
fn inn_trigger_chain_reaches_set_stage() {
    if !present() {
        eprintln!("skipping real-script test: scripts dir not present");
        return;
    }
    let mut vm = Vm::new();
    load_many(
        &mut vm,
        &[
            "mq106innroomtriggerscript",
            "trigger",
            "mq106script",
            "quest",
            "actor",
            "objectreference",
            "form",
        ],
    );
    let trigger = vm
        .create_instance(0x9002, "MQ106InnRoomTriggerScript", None)
        .unwrap();
    let player = vm.create_instance(0x9003, "Actor", None).unwrap();
    let mq106 = vm.create_instance(0x9004, "MQ106Script", None).unwrap();
    vm.set_player(player);
    assert!(vm.run_until_idle(1000, 100));
    vm.instances
        .get_mut(trigger)
        .unwrap()
        .set("::MQ106_var", RuntimeValue::Object(mq106));

    let errors_before = vm.errors.len();
    assert!(vm.post_event(trigger, "OnLoad", vec![]));
    assert!(vm.run_until_idle(1000, 100));
    assert_eq!(vm.errors.len(), errors_before, "OnLoad produced errors");
    let target_ref = vm
        .instances
        .get(trigger)
        .and_then(|i| i.get("::targetRef_var").cloned());
    assert_eq!(target_ref, Some(RuntimeValue::Object(player)));

    let errors_before = vm.errors.len();
    assert!(vm.post_event(
        trigger,
        "OnTriggerEnter",
        vec![RuntimeValue::Object(player)]
    ));
    assert!(vm.run_until_idle(1000, 100));
    let in_room = vm
        .instances
        .get(mq106)
        .and_then(|i| i.get("::playerInRoom_var").cloned());
    assert_eq!(in_room, Some(RuntimeValue::Bool(true)));
    let new_errors = &vm.errors[errors_before..];
    assert_eq!(
        new_errors.len(),
        1,
        "expected exactly one error: {new_errors:?}"
    );
    assert!(
        matches!(
            &new_errors[0],
            RuntimeError::MissingNative { script, function }
                if script == "Quest" && function == "SetCurrentStageID"
        ),
        "unexpected error: {:?}",
        new_errors[0]
    );

    let errors_before = vm.errors.len();
    assert!(vm.post_event(
        trigger,
        "OnTriggerLeave",
        vec![RuntimeValue::Object(player)]
    ));
    assert!(vm.run_until_idle(1000, 100));
    let in_room = vm
        .instances
        .get(mq106)
        .and_then(|i| i.get("::playerInRoom_var").cloned());
    assert_eq!(in_room, Some(RuntimeValue::Bool(false)));
    let new_errors = &vm.errors[errors_before..];
    assert_eq!(
        new_errors.len(),
        1,
        "expected exactly one error: {new_errors:?}"
    );
    assert!(
        matches!(&new_errors[0], RuntimeError::CallOnNone { what } if what.contains("GetActorRef")),
        "unexpected error: {:?}",
        new_errors[0]
    );
}

/// OnInit census over all real scripts: no OnInit may raise a
/// type error (a VM bug), and at least the M7.1 baseline of 106
/// must run clean. The bounds are one-sided on purpose — adding
/// natives can only move scripts from errored to clean.
#[test]
fn census_oninit_has_no_type_errors() {
    if !present() {
        eprintln!("skipping real-script test: scripts dir not present");
        return;
    }
    let mut vm = Vm::new();
    let mut paths: Vec<PathBuf> = Vec::new();
    for entry in fs::read_dir(scripts_dir()).expect("read scripts dir") {
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
    let mut names = Vec::new();
    for path in &paths {
        let bytes = fs::read(path).expect("read pex");
        let file = PexFile::from_bytes(&bytes).expect("parse pex");
        for object in &file.objects {
            names.push(object.name.clone());
        }
        vm.load_pex(&file);
    }
    names.sort();
    names.dedup();
    assert_eq!(
        names.len(),
        13_494,
        "script count changed — update the pins"
    );

    let mut clean = 0usize;
    let mut errored = 0usize;
    for (i, script) in names.iter().enumerate() {
        let form = 0x10000 + i as u32;
        let auto_state = vm
            .scripts
            .object(script)
            .map(|o| o.auto_state.clone())
            .unwrap_or_default();
        if resolve_function(&vm.scripts, script, &auto_state, "OnInit").is_none() {
            continue;
        }
        let errors_before = vm.errors.len();
        let Ok(_id) = vm.create_instance(form, script, None) else {
            continue;
        };
        assert!(
            vm.run_until_idle(1000, 100),
            "{script} OnInit did not drain"
        );
        let new_errors = &vm.errors[errors_before..];
        for e in new_errors {
            assert!(
                !matches!(e, RuntimeError::TypeMismatch { .. }),
                "{script} OnInit raised a type error (VM bug): {e}"
            );
        }
        if new_errors.is_empty() {
            clean += 1;
        } else {
            errored += 1;
        }
    }
    assert!(
        clean >= 106,
        "only {clean} OnInits ran clean (baseline 106)"
    );
    assert!(errored <= 77, "{errored} OnInits errored (baseline 77)");
}
