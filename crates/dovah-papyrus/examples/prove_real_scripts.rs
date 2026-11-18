//! M7.1 proving pass (design doc §7.8): run real game scripts
//! and report what happens.
//!
//! Usage: `cargo run -p dovah-papyrus --example
//! prove_real_scripts -- <scripts-dir>`
//!
//! 1. Pure functions with known answers: `CWScript.HoldName`
//!    (all nine holds plus the fall-through) and
//!    `CraftSmithing.GetMinSkill` (the steel branch plus the
//!    fall-through).
//! 2. The Sleeping Giant Inn's five attached scripts: run each
//!    `OnInit` and report trace output and errors.
//! 3. OnInit census: create an instance of every script in the
//!    directory, run its `OnInit`, and report how many finish
//!    clean, park on a latent call, or error — with the missing
//!    natives ranked by how many scripts hit them.
//! 4. Graduation attempt: drive the inn trigger and a
//!    set-stage-on-enter trigger with real instances, showing
//!    exactly which natives block in-game behavior.
//!
//! Nothing here writes game data; instances live only in memory.

use dovah_papyrus::resolve::resolve_function;
use dovah_papyrus::value::RuntimeValue;
use dovah_papyrus::Vm;
use dovah_pex::PexFile;
use std::collections::{BTreeMap, HashMap};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn load_dir(vm: &mut Vm, dir: &Path) -> Vec<String> {
    let mut paths: Vec<PathBuf> = Vec::new();
    for entry in fs::read_dir(dir).expect("read scripts dir") {
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
    names
}

fn call_int(vm: &mut Vm, id: dovah_papyrus::InstanceId, name: &str, arg: i32) -> RuntimeValue {
    assert!(vm.call_function(id, name, vec![RuntimeValue::Int(arg)]));
    assert!(vm.run_until_idle(1000, 100));
    vm.last_stack_result.clone().unwrap_or(RuntimeValue::None)
}

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let Some(dir) = args.next() else {
        eprintln!("usage: prove_real_scripts <scripts-dir>");
        return ExitCode::from(2);
    };
    let mut vm = Vm::new();
    let names = load_dir(&mut vm, Path::new(&dir));
    println!("loaded {} scripts", names.len());

    // ---- 1. pure functions ----
    println!("\n== pure functions ==");
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
    let mut hold_pass = 0;
    for (number, expected) in holds {
        let got = call_int(&mut vm, cw, "HoldName", number);
        let ok = got == RuntimeValue::Str(std::rc::Rc::from(expected));
        if ok {
            hold_pass += 1;
        }
        println!("  HoldName({number}) = {got:?} (expected \"{expected}\") {ok}");
    }
    println!("  HoldName: {hold_pass}/{} correct", holds.len());

    let smith = vm.create_instance(0x1001, "CraftSmithing", None).unwrap();
    assert!(vm.run_until_idle(1000, 100));
    let steel = vm.create_instance(0x2000, "Keyword", None).unwrap();
    let other = vm.create_instance(0x2001, "Keyword", None).unwrap();
    assert!(vm.run_until_idle(1000, 100));
    vm.instances
        .get_mut(smith)
        .unwrap()
        .set("::WeaponMaterialSteel_var", RuntimeValue::Object(steel));
    assert!(vm.call_function(smith, "GetMinSkill", vec![RuntimeValue::Object(steel)]));
    assert!(vm.run_until_idle(1000, 100));
    println!(
        "  GetMinSkill(steel) = {:?} (expected Int(5))",
        vm.last_stack_result
    );
    assert!(vm.call_function(smith, "GetMinSkill", vec![RuntimeValue::Object(other)]));
    assert!(vm.run_until_idle(1000, 100));
    println!(
        "  GetMinSkill(other) = {:?} (expected Int(0))",
        vm.last_stack_result
    );

    // ---- 2. the inn's scripts ----
    println!("\n== Sleeping Giant Inn scripts (OnInit) ==");
    let inn_scripts = [
        "defaultOnEnter",
        "defaultPlayerEnableDisableLinkedRef",
        "defaultsetstageonenter",
        "defaultStartSceneTrigScript",
        "MQ106InnRoomTriggerScript",
    ];
    for (i, script) in inn_scripts.iter().enumerate() {
        let errors_before = vm.errors.len();
        let traces_before = vm.trace_log.len();
        let id = match vm.create_instance(0x3000 + i as u32, script, None) {
            Ok(id) => id,
            Err(e) => {
                println!("  {script}: create failed: {e}");
                continue;
            }
        };
        let drained = vm.run_until_idle(1000, 100);
        let new_errors = &vm.errors[errors_before..];
        let new_traces = &vm.trace_log[traces_before..];
        let target_total = vm
            .instances
            .get(id)
            .and_then(|inst| inst.get("targetCountTotal").cloned());
        println!(
            "  {script}: drained={drained} errors={} traces={} targetCountTotal={target_total:?}",
            new_errors.len(),
            new_traces.len()
        );
        for e in new_errors {
            println!("    error: {e}");
        }
        for t in new_traces {
            println!("    trace: {t}");
        }
    }

    // ---- 3. OnInit census ----
    println!("\n== OnInit census (all scripts) ==");
    let mut no_oninit = 0usize;
    let mut clean = 0usize;
    let mut parked = 0usize;
    let mut errored = 0usize;
    let mut stuck = 0usize;
    let mut missing_natives: HashMap<(String, String), usize> = HashMap::new();
    let mut other_errors: BTreeMap<String, usize> = BTreeMap::new();
    let mut other_error_detail: Vec<(String, String)> = Vec::new();
    for (i, script) in names.iter().enumerate() {
        let form = 0x10000 + i as u32;
        if vm.instances.find(form, script).is_some() {
            continue;
        }
        let auto_state = vm
            .scripts
            .object(script)
            .map(|o| o.auto_state.clone())
            .unwrap_or_default();
        let has_oninit = resolve_function(&vm.scripts, script, &auto_state, "OnInit").is_some();
        if !has_oninit {
            no_oninit += 1;
            continue;
        }
        let errors_before = vm.errors.len();
        let pending_before = vm.pending_stacks();
        let Ok(_id) = vm.create_instance(form, script, None) else {
            continue;
        };
        let drained = vm.run_until_idle(1000, 100);
        if !drained {
            stuck += 1;
            vm.drop_runnable_stacks();
        }
        let new_errors = &vm.errors[errors_before..];
        if !new_errors.is_empty() {
            errored += 1;
            for e in new_errors {
                match e {
                    dovah_papyrus::error::RuntimeError::MissingNative {
                        script: s,
                        function: f,
                    } => {
                        *missing_natives.entry((s.clone(), f.clone())).or_insert(0) += 1;
                    }
                    other => {
                        let kind = format!("{other:?}")
                            .split(['(', '{'])
                            .next()
                            .unwrap_or("?")
                            .to_string();
                        *other_errors.entry(kind).or_insert(0) += 1;
                        other_error_detail.push((script.clone(), format!("{other}")));
                    }
                }
            }
        } else if vm.pending_stacks() > pending_before {
            parked += 1;
        } else {
            clean += 1;
        }
    }
    println!("  scripts: {}", names.len());
    println!("  no OnInit: {no_oninit}");
    println!("  OnInit clean: {clean}");
    println!("  OnInit parked on a latent call: {parked}");
    println!("  OnInit errored: {errored}");
    println!("  OnInit did not finish its budget: {stuck}");
    println!("  total trace lines: {}", vm.trace_log.len());
    println!("  total notifications: {}", vm.notifications.len());
    println!(
        "  total instructions executed: {}",
        vm.instructions_executed
    );
    let mut missing: Vec<(&(String, String), &usize)> = missing_natives.iter().collect();
    missing.sort_by(|a, b| b.1.cmp(a.1));
    println!("  missing natives hit (top 20):");
    for ((script, function), count) in missing.iter().take(20) {
        println!("    {script}::{function}: {count} scripts");
    }
    println!("  distinct missing natives: {}", missing_natives.len());
    println!("  other error kinds:");
    for (kind, count) in &other_errors {
        println!("    {kind}: {count}");
    }
    for (script, error) in &other_error_detail {
        println!("    {script}: {error}");
    }

    // ---- 4. graduation attempt ----
    println!("\n== graduation attempt ==");
    // defaultSetStageOnEnter.TriggerMe with a real quest instance.
    let trigger = vm
        .create_instance(0x9000, "defaultsetstageonenter", None)
        .unwrap();
    let quest = vm.create_instance(0x9001, "Quest", None).unwrap();
    assert!(vm.run_until_idle(1000, 100));
    vm.instances
        .get_mut(trigger)
        .unwrap()
        .set("::myQuest_var", RuntimeValue::Object(quest));
    vm.instances
        .get_mut(trigger)
        .unwrap()
        .set("::stage_var", RuntimeValue::Int(20));
    let errors_before = vm.errors.len();
    assert!(vm.call_function(trigger, "TriggerMe", vec![]));
    assert!(vm.run_until_idle(1000, 100));
    println!("  defaultSetStageOnEnter.TriggerMe (quest wired, stage 20):");
    for e in &vm.errors[errors_before..] {
        println!("    error: {e}");
    }
    if vm.errors.len() == errors_before {
        println!("    no errors — the trigger ran clean");
    }
    // MQ106 inn-room trigger: the full in-game chain. OnLoad
    // wires the target ref to the player (Game.GetPlayer);
    // entering the trigger fires the room event, which sets
    // playerInRoom on the MQ106 quest script and advances the
    // quest to stage 20. (::MQ106_var is wired directly here;
    // in the real game the ESM VMAD does it.)
    let mq_trigger = vm
        .create_instance(0x9002, "MQ106InnRoomTriggerScript", None)
        .unwrap();
    let player = vm.create_instance(0x9003, "Actor", None).unwrap();
    let mq106 = vm.create_instance(0x9004, "MQ106Script", None).unwrap();
    vm.set_player(player);
    assert!(vm.run_until_idle(1000, 100));
    vm.instances
        .get_mut(mq_trigger)
        .unwrap()
        .set("::MQ106_var", RuntimeValue::Object(mq106));
    let errors_before = vm.errors.len();
    assert!(vm.post_event(mq_trigger, "OnLoad", vec![]));
    assert!(vm.run_until_idle(1000, 100));
    let target_ref = vm
        .instances
        .get(mq_trigger)
        .and_then(|i| i.get("::targetRef_var").cloned());
    println!(
        "  after OnLoad: ::targetRef_var is the player: {}",
        matches!(target_ref, Some(RuntimeValue::Object(id)) if id == player)
    );
    let posted = vm.post_event(
        mq_trigger,
        "OnTriggerEnter",
        vec![RuntimeValue::Object(player)],
    );
    println!("  OnTriggerEnter(player): posted={posted}");
    assert!(vm.run_until_idle(1000, 100));
    let in_room = vm
        .instances
        .get(mq106)
        .and_then(|i| i.get("::playerInRoom_var").cloned());
    println!("  after enter: MQ106Script.playerInRoom = {in_room:?} (expected Bool(true))");
    for e in &vm.errors[errors_before..] {
        println!("    error: {e}");
    }
    // Leaving: the else branch. Delphine's alias is not wired
    // (no VMAD), so GetActorRef runs on None.
    let errors_before = vm.errors.len();
    let posted = vm.post_event(
        mq_trigger,
        "OnTriggerLeave",
        vec![RuntimeValue::Object(player)],
    );
    println!("  OnTriggerLeave(player): posted={posted}");
    assert!(vm.run_until_idle(1000, 100));
    let in_room = vm
        .instances
        .get(mq106)
        .and_then(|i| i.get("::playerInRoom_var").cloned());
    println!("  after leave: MQ106Script.playerInRoom = {in_room:?} (expected Bool(false))");
    for e in &vm.errors[errors_before..] {
        println!("    error: {e}");
    }

    ExitCode::SUCCESS
}
