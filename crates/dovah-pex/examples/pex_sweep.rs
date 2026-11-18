//! Parse-sweep every `.pex` file in a directory and report the
//! aggregate results: parse success/failure by kind, opcode
//! histogram, jump-label statistics (the absolute-vs-relative
//! question), call-operand shape conformance and resolution (the
//! call-order question), native declarations, call-frequency
//! tables, and pure-function candidates for the proving pass.
//!
//! Usage: `cargo run -p dovah-pex --example pex_sweep -- <dir>`
//!
//! Parses one file at a time and keeps only aggregates, so memory
//! stays small on a 13k-file tree. Built for M7.1.

use dovah_pex::{opcode_name, Function, PexError, PexFile, Value};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// One declared function, for the cross-file call-resolution check.
#[derive(Debug, Clone, Copy)]
struct FnInfo {
    native: bool,
    params: usize,
}

/// Call-frequency tables: (count, distinct arg counts) per call
/// target, keyed by lowercased names.
#[derive(Default)]
struct CallTables {
    static_calls: HashMap<(String, String), (u64, BTreeSet<u8>)>,
    method_calls: HashMap<String, (u64, BTreeSet<u8>)>,
    parent_calls: HashMap<(String, String), (u64, BTreeSet<u8>)>,
}

#[derive(Default)]
struct JumpExample {
    file: String,
    function: String,
    pc: usize,
    label: i32,
    count: usize,
}

#[derive(Default)]
struct Stats {
    files_total: usize,
    files_ok: usize,
    failures: BTreeMap<String, (usize, Vec<String>)>,
    versions: BTreeMap<(u8, u8), usize>,
    game_ids: BTreeMap<u16, usize>,
    objects: usize,
    functions: usize,
    functions_native: usize,
    functions_global: usize,
    functions_with_bytecode: usize,
    instructions: usize,
    opcode_hist: Vec<u64>,
    /// First observed example of each opcode, for layout checks.
    opcode_examples: Vec<Option<String>>,
    // Jump analysis.
    jumps_total: usize,
    jumps_negative_label: usize,
    jumps_label_past_end: usize, // label > count (impossible for absolute)
    jumps_label_at_end: usize,   // label == count (fall off the end)
    jumps_abs_valid: usize,
    jumps_abs_invalid: usize,
    jumps_rel_valid: usize, // pc + label in [0, count]
    jumps_rel_invalid: usize,
    jumps_both_valid: usize,
    jumps_neither_valid: usize,
    jump_arg_bad_shape: usize, // label arg not an Int, or condition not an Identifier
    neg_examples: Vec<JumpExample>,
    past_end_examples: Vec<JumpExample>,
    loop_examples: Vec<JumpExample>, // negative label = backward jump (a loop)
    bad_jump_examples: Vec<String>,
    // Call shape conformance (proven layout: method first, dest
    // last — see docs/formats/pex.md).
    callmethod_total: usize,
    callmethod_bad_dest: usize,
    callmethod_bad_method: usize,
    callparent_total: usize,
    callparent_bad_dest: usize,
    callparent_bad_method: usize,
    callparent_examples: Vec<String>,
    callstatic_total: usize,
    callstatic_bad_dest: usize,
    callstatic_bad_script: usize,
    callstatic_bad_method: usize,
    // Pure-function candidates for the proving pass.
    pure_functions: usize,
    pure_candidates: Vec<(String, String, usize, String, bool)>, // script, fn, instrs, ret, has_loop
}

fn is_identifier(value: &Value) -> bool {
    matches!(value, Value::Identifier(_))
}

fn is_dest(value: &Value) -> bool {
    matches!(value, Value::None | Value::Identifier(_))
}

/// Classify a parse error for the failure table.
fn failure_kind(error: &PexError) -> String {
    match error {
        PexError::Io(_) => "Io".to_string(),
        PexError::BadMagic => "BadMagic".to_string(),
        PexError::UnsupportedVersion(major, minor) => {
            format!("UnsupportedVersion({major}.{minor})")
        }
        PexError::Truncated(what) => format!("Truncated({what})"),
        PexError::InvalidData(what) => format!("InvalidData({what})"),
        PexError::BadStringIndex(_) => "BadStringIndex".to_string(),
        PexError::UnknownOpcode(op) => format!("UnknownOpcode(0x{op:02x})"),
    }
}

fn analyze_function(
    stats: &mut Stats,
    file_name: &str,
    script_name: &str,
    fn_name: &str,
    function: &Function,
    calls: &mut CallTables,
) {
    stats.functions += 1;
    if function.is_native() {
        stats.functions_native += 1;
    }
    if function.is_global() {
        stats.functions_global += 1;
    }
    if function.is_native() {
        return;
    }
    stats.functions_with_bytecode += 1;
    let count = function.instructions.len();
    let mut pure = true;
    let mut has_loop = false;
    for (pc, instruction) in function.instructions.iter().enumerate() {
        stats.instructions += 1;
        if (instruction.op as usize) < 36 {
            stats.opcode_hist[instruction.op as usize] += 1;
            let slot = &mut stats.opcode_examples[instruction.op as usize];
            if slot.is_none() {
                *slot = Some(format!(
                    "{file_name} {script_name}.{fn_name} pc={pc} args={:?}",
                    instruction.args
                ));
            }
        }
        match instruction.op {
            0x14..=0x16 => {
                // jmp L / jmpt cond L / jmpf cond L
                stats.jumps_total += 1;
                let label_value = if instruction.op == 0x14 {
                    instruction.args.first()
                } else {
                    instruction.args.get(1)
                };
                let cond_ok =
                    instruction.op == 0x14 || instruction.args.first().is_some_and(is_identifier);
                let Some(Value::Int(label)) = label_value else {
                    stats.jump_arg_bad_shape += 1;
                    if stats.bad_jump_examples.len() < 3 {
                        stats.bad_jump_examples.push(format!(
                            "{file_name} {script_name}.{fn_name} pc={pc} op={} args={:?}",
                            instruction.op, instruction.args
                        ));
                    }
                    continue;
                };
                if !cond_ok {
                    stats.jump_arg_bad_shape += 1;
                    if stats.bad_jump_examples.len() < 3 {
                        stats.bad_jump_examples.push(format!(
                            "{file_name} {script_name}.{fn_name} pc={pc} op={} args={:?}",
                            instruction.op, instruction.args
                        ));
                    }
                }
                let label = *label;
                let abs_valid = (0..=count as i64).contains(&(label as i64));
                let rel_target = pc as i64 + label as i64;
                let rel_valid = (0..=count as i64).contains(&rel_target);
                if abs_valid {
                    stats.jumps_abs_valid += 1;
                } else {
                    stats.jumps_abs_invalid += 1;
                }
                if rel_valid {
                    stats.jumps_rel_valid += 1;
                } else {
                    stats.jumps_rel_invalid += 1;
                }
                match (abs_valid, rel_valid) {
                    (true, true) => stats.jumps_both_valid += 1,
                    (false, false) => stats.jumps_neither_valid += 1,
                    _ => {}
                }
                if label < 0 {
                    stats.jumps_negative_label += 1;
                    if stats.neg_examples.len() < 5 {
                        stats.neg_examples.push(JumpExample {
                            file: file_name.to_string(),
                            function: format!("{script_name}.{fn_name}"),
                            pc,
                            label,
                            count,
                        });
                    }
                } else if label as usize > count {
                    stats.jumps_label_past_end += 1;
                    if stats.past_end_examples.len() < 5 {
                        stats.past_end_examples.push(JumpExample {
                            file: file_name.to_string(),
                            function: format!("{script_name}.{fn_name}"),
                            pc,
                            label,
                            count,
                        });
                    }
                } else if label as usize == count {
                    stats.jumps_label_at_end += 1;
                }
                // A negative label is a backward jump = a loop.
                if label < 0 {
                    has_loop = true;
                    if stats.loop_examples.len() < 5 {
                        stats.loop_examples.push(JumpExample {
                            file: file_name.to_string(),
                            function: format!("{script_name}.{fn_name}"),
                            pc,
                            label,
                            count,
                        });
                    }
                }
            }
            0x17 => {
                // callmethod method, object, dest, args...
                pure = false;
                stats.callmethod_total += 1;
                if !instruction.args.get(2).is_some_and(is_dest) {
                    stats.callmethod_bad_dest += 1;
                }
                match instruction.args.first() {
                    Some(Value::Identifier(method)) => {
                        let entry = calls
                            .method_calls
                            .entry(method.to_ascii_lowercase())
                            .or_insert((0, BTreeSet::new()));
                        entry.0 += 1;
                        entry
                            .1
                            .insert((instruction.args.len().saturating_sub(3)) as u8);
                    }
                    _ => stats.callmethod_bad_method += 1,
                }
            }
            0x18 => {
                // callparent method, dest, args...
                pure = false;
                stats.callparent_total += 1;
                if !instruction.args.get(1).is_some_and(is_dest) {
                    stats.callparent_bad_dest += 1;
                }
                if stats.callparent_examples.len() < 3 {
                    stats.callparent_examples.push(format!(
                        "{file_name} {script_name}.{fn_name} pc={pc} args={:?}",
                        instruction.args
                    ));
                }
                match instruction.args.first() {
                    Some(Value::Identifier(method)) => {
                        let entry = calls
                            .parent_calls
                            .entry((
                                script_name.to_ascii_lowercase(),
                                method.to_ascii_lowercase(),
                            ))
                            .or_insert((0, BTreeSet::new()));
                        entry.0 += 1;
                        entry
                            .1
                            .insert((instruction.args.len().saturating_sub(2)) as u8);
                    }
                    _ => stats.callparent_bad_method += 1,
                }
            }
            0x19 => {
                // callstatic script, method, dest, args...
                pure = false;
                stats.callstatic_total += 1;
                if !instruction.args.get(2).is_some_and(is_dest) {
                    stats.callstatic_bad_dest += 1;
                }
                let script_ok = instruction.args.first().is_some_and(is_identifier);
                let method_ok = instruction.args.get(1).is_some_and(is_identifier);
                if !script_ok {
                    stats.callstatic_bad_script += 1;
                }
                if !method_ok {
                    stats.callstatic_bad_method += 1;
                }
                if let (Some(Value::Identifier(script)), Some(Value::Identifier(method))) =
                    (instruction.args.first(), instruction.args.get(1))
                {
                    let entry = calls
                        .static_calls
                        .entry((script.to_ascii_lowercase(), method.to_ascii_lowercase()))
                        .or_insert((0, BTreeSet::new()));
                    entry.0 += 1;
                    entry
                        .1
                        .insert((instruction.args.len().saturating_sub(3)) as u8);
                }
            }
            0x1C | 0x1D => {
                // propget / propset touch instance state: not pure.
                pure = false;
            }
            _ => {}
        }
    }
    if pure && count >= 5 && function.return_type != "None" {
        stats.pure_functions += 1;
        if stats.pure_candidates.len() < 100 {
            stats.pure_candidates.push((
                script_name.to_string(),
                fn_name.to_string(),
                count,
                function.return_type.clone(),
                has_loop,
            ));
        }
    }
}

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let Some(dir) = args.next() else {
        eprintln!("usage: pex_sweep <dir-of-pex-files>");
        return ExitCode::from(2);
    };
    let dir_path = Path::new(&dir);
    let mut paths: Vec<PathBuf> = Vec::new();
    let entries = match fs::read_dir(dir_path) {
        Ok(entries) => entries,
        Err(e) => {
            eprintln!("cannot read {dir}: {e}");
            return ExitCode::from(1);
        }
    };
    for entry in entries {
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

    let mut stats = Stats {
        opcode_hist: vec![0; 36],
        opcode_examples: vec![None; 36],
        ..Stats::default()
    };
    // Script index for the call-resolution check: lowercased script
    // -> (lowercased parent, functions by lowercased name).
    let mut parents: HashMap<String, String> = HashMap::new();
    let mut functions: HashMap<String, HashMap<String, FnInfo>> = HashMap::new();
    let mut calls = CallTables::default();
    let mut native_decls: HashMap<String, Vec<(String, usize)>> = HashMap::new();

    for path in &paths {
        stats.files_total += 1;
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) => {
                let entry = stats
                    .failures
                    .entry(format!("Io({e})"))
                    .or_insert((0, Vec::new()));
                entry.0 += 1;
                if entry.1.len() < 3 {
                    entry.1.push(file_name);
                }
                continue;
            }
        };
        let file = match PexFile::from_bytes(&bytes) {
            Ok(file) => file,
            Err(e) => {
                let entry = stats
                    .failures
                    .entry(failure_kind(&e))
                    .or_insert((0, Vec::new()));
                entry.0 += 1;
                if entry.1.len() < 3 {
                    entry.1.push(file_name);
                }
                continue;
            }
        };
        stats.files_ok += 1;
        *stats
            .versions
            .entry((file.major_version, file.minor_version))
            .or_insert(0) += 1;
        *stats.game_ids.entry(file.game_id).or_insert(0) += 1;
        for object in &file.objects {
            stats.objects += 1;
            let script_key = object.name.to_ascii_lowercase();
            parents.insert(script_key.clone(), object.parent.to_ascii_lowercase());
            let fn_map = functions.entry(script_key.clone()).or_default();
            for state in &object.states {
                for named in &state.functions {
                    fn_map
                        .entry(named.name.to_ascii_lowercase())
                        .or_insert(FnInfo {
                            native: named.function.is_native(),
                            params: named.function.params.len(),
                        });
                    if named.function.is_native() {
                        native_decls
                            .entry(script_key.clone())
                            .or_default()
                            .push((named.name.clone(), named.function.params.len()));
                    }
                    analyze_function(
                        &mut stats,
                        &file_name,
                        &object.name,
                        &named.name,
                        &named.function,
                        &mut calls,
                    );
                }
            }
            for property in &object.properties {
                if let Some(handler) = &property.read_handler {
                    analyze_function(
                        &mut stats,
                        &file_name,
                        &object.name,
                        &format!("{}.get", property.name),
                        handler,
                        &mut calls,
                    );
                }
                if let Some(handler) = &property.write_handler {
                    analyze_function(
                        &mut stats,
                        &file_name,
                        &object.name,
                        &format!("{}.set", property.name),
                        handler,
                        &mut calls,
                    );
                }
            }
        }
    }

    // ---- callstatic resolution against the completed index ----
    let resolve = |script: &str, method: &str| -> Option<(FnInfo, bool)> {
        let mut current = Some(script.to_string());
        let mut seen: HashSet<String> = HashSet::new();
        while let Some(name) = current {
            if !seen.insert(name.clone()) {
                return None; // inheritance cycle
            }
            if let Some(map) = functions.get(&name) {
                if let Some(info) = map.get(method) {
                    return Some((*info, name != script));
                }
            }
            current = parents.get(&name).filter(|p| !p.is_empty()).cloned();
        }
        None
    };
    let mut static_resolved: u64 = 0;
    let mut static_unresolved: u64 = 0;
    let mut static_argc_match: u64 = 0;
    let mut static_argc_mismatch: u64 = 0;
    let mut unresolved_examples: Vec<((String, String), u64)> = Vec::new();
    let mut argc_mismatch_examples: Vec<((String, String), u8, usize, u64)> = Vec::new();
    for ((script, method), (count, argcs)) in &calls.static_calls {
        match resolve(script, method) {
            Some((info, _)) => {
                static_resolved += count;
                for argc in argcs {
                    if *argc as usize == info.params {
                        static_argc_match += count;
                    } else {
                        static_argc_mismatch += count;
                        argc_mismatch_examples.push((
                            (script.clone(), method.clone()),
                            *argc,
                            info.params,
                            *count,
                        ));
                    }
                }
            }
            None => {
                static_unresolved += count;
                unresolved_examples.push(((script.clone(), method.clone()), *count));
            }
        }
    }
    // callparent resolution: the callee lives on the enclosing
    // script's parent chain.
    let mut parent_resolved: u64 = 0;
    let mut parent_unresolved: u64 = 0;
    let mut parent_argc_match: u64 = 0;
    let mut parent_argc_mismatch: u64 = 0;
    for ((script, method), (count, argcs)) in &calls.parent_calls {
        let parent = parents.get(script).cloned().unwrap_or_default();
        match resolve(&parent, method) {
            Some((info, _)) => {
                parent_resolved += count;
                for argc in argcs {
                    if *argc as usize == info.params {
                        parent_argc_match += count;
                    } else {
                        parent_argc_mismatch += count;
                    }
                }
            }
            None => parent_unresolved += count,
        }
    }

    // ---- report ----
    println!("== parse ==");
    println!(
        "files: {} total, {} ok, {} failed",
        stats.files_total,
        stats.files_ok,
        stats.files_total - stats.files_ok
    );
    for (kind, (count, examples)) in &stats.failures {
        println!("  fail {kind}: {count} (e.g. {})", examples.join(", "));
    }
    println!("versions: {:?}", stats.versions);
    println!("game ids: {:?}", stats.game_ids);
    println!(
        "objects: {}, functions: {} ({} native, {} global, {} with bytecode)",
        stats.objects,
        stats.functions,
        stats.functions_native,
        stats.functions_global,
        stats.functions_with_bytecode
    );
    println!("instructions: {}", stats.instructions);
    print!("opcode histogram:");
    for (op, count) in stats.opcode_hist.iter().enumerate() {
        if *count > 0 {
            print!(" {}={}", opcode_name(op as u8).unwrap_or("?"), count);
        }
    }
    println!();
    for (op, example) in stats.opcode_examples.iter().enumerate() {
        if let Some(example) = example {
            println!(
                "  example {}: {}",
                opcode_name(op as u8).unwrap_or("?"),
                example
            );
        }
    }

    println!("\n== jumps (absolute vs relative) ==");
    println!("jump instructions: {}", stats.jumps_total);
    println!(
        "  negative labels (impossible for absolute): {}",
        stats.jumps_negative_label
    );
    println!(
        "  labels past the end (impossible for absolute): {}",
        stats.jumps_label_past_end
    );
    println!(
        "  labels exactly at the end (fall off): {}",
        stats.jumps_label_at_end
    );
    println!("  bad jump arg shape: {}", stats.jump_arg_bad_shape);
    println!(
        "  valid under absolute: {}, invalid under absolute: {}",
        stats.jumps_abs_valid, stats.jumps_abs_invalid
    );
    println!(
        "  valid under relative-to-jump: {}, invalid: {}",
        stats.jumps_rel_valid, stats.jumps_rel_invalid
    );
    println!(
        "  valid under both: {}, valid under neither: {}",
        stats.jumps_both_valid, stats.jumps_neither_valid
    );
    for ex in &stats.neg_examples {
        println!(
            "  NEG  {} {} pc={} label={} count={}",
            ex.file, ex.function, ex.pc, ex.label, ex.count
        );
    }
    for ex in &stats.past_end_examples {
        println!(
            "  PAST {} {} pc={} label={} count={}",
            ex.file, ex.function, ex.pc, ex.label, ex.count
        );
    }
    for ex in &stats.loop_examples {
        println!(
            "  LOOP {} {} pc={} label={} count={}",
            ex.file, ex.function, ex.pc, ex.label, ex.count
        );
    }

    for ex in &stats.bad_jump_examples {
        println!("  BADJUMP {ex}");
    }
    for ex in &stats.callparent_examples {
        println!("  CALLPARENT {ex}");
    }

    println!("\n== call operand shape (method-first, dest-last layout) ==");
    println!(
        "callmethod: {} total, {} bad dest, {} bad method",
        stats.callmethod_total, stats.callmethod_bad_dest, stats.callmethod_bad_method
    );
    println!(
        "callparent: {} total, {} bad dest, {} bad method",
        stats.callparent_total, stats.callparent_bad_dest, stats.callparent_bad_method
    );
    println!(
        "callstatic: {} total, {} bad dest, {} bad script, {} bad method",
        stats.callstatic_total,
        stats.callstatic_bad_dest,
        stats.callstatic_bad_script,
        stats.callstatic_bad_method
    );
    println!(
        "callstatic resolution: {static_resolved} resolved, {static_unresolved} unresolved \
         (distinct targets: {})",
        calls.static_calls.len()
    );
    println!(
        "callstatic arg count vs declared params: {static_argc_match} match, \
         {static_argc_mismatch} mismatch"
    );
    unresolved_examples.sort_by_key(|a| Reverse(a.1));
    for ((script, method), count) in unresolved_examples.iter().take(10) {
        println!("  UNRESOLVED {script}::{method}: {count} calls");
    }
    argc_mismatch_examples.sort_by_key(|a| Reverse(a.3));
    for ((script, method), argc, params, count) in argc_mismatch_examples.iter().take(10) {
        println!(
            "  ARGC {script}::{method}: called with {argc} args, declares {params} params ({count} calls)"
        );
    }
    println!(
        "callparent resolution: {parent_resolved} resolved, {parent_unresolved} unresolved; \
         arg count: {parent_argc_match} match, {parent_argc_mismatch} mismatch"
    );

    println!(
        "\n== all callstatic targets ({} distinct) ==",
        calls.static_calls.len()
    );
    let mut targets: Vec<_> = calls.static_calls.iter().collect();
    targets.sort_by_key(|a| Reverse(a.1 .0));
    for ((script, method), (count, argcs)) in targets.iter() {
        let kind = match resolve(script, method) {
            Some((info, _)) => {
                if info.native {
                    "native"
                } else {
                    "script"
                }
            }
            None => "MISSING",
        };
        let argc_list: Vec<String> = argcs.iter().map(u8::to_string).collect();
        println!(
            "  {script}::{method}: {count} calls [{kind}] argcounts={}",
            argc_list.join(",")
        );
    }

    println!("\n== top callmethod names ==");
    let mut methods: Vec<_> = calls.method_calls.iter().collect();
    methods.sort_by_key(|a| Reverse(a.1 .0));
    for (method, (count, argcs)) in methods.iter().take(30) {
        let argc_list: Vec<String> = argcs.iter().map(u8::to_string).collect();
        println!(
            "  {method}: {count} calls argcounts={}",
            argc_list.join(",")
        );
    }

    println!("\n== native declarations ==");
    let total_natives: usize = native_decls.values().map(Vec::len).sum();
    println!(
        "declared native functions: {total_natives} across {} scripts",
        native_decls.len()
    );
    let mut by_script: Vec<(&String, &Vec<(String, usize)>)> = native_decls.iter().collect();
    by_script.sort_by_key(|a| Reverse(a.1.len()));
    for (script, decls) in by_script.iter().take(15) {
        println!("  {script}: {} natives", decls.len());
    }
    for script in ["debug", "utility", "game", "math", "stringutil", "script"] {
        if let Some(decls) = native_decls.get(script) {
            let mut sorted = decls.clone();
            sorted.sort();
            sorted.dedup();
            let list: Vec<String> = sorted
                .iter()
                .map(|(name, params)| format!("{name}/{params}"))
                .collect();
            println!("  {script}: {}", list.join(", "));
        }
    }

    println!("\n== pure functions (no calls, no property access) ==");
    println!(
        "pure functions returning a value, >= 5 instructions: {}",
        stats.pure_functions
    );
    let mut candidates = stats.pure_candidates.clone();
    candidates.sort_by_key(|a| Reverse(a.2));
    for (script, name, instrs, ret, has_loop) in candidates.iter().take(25) {
        println!(
            "  {script}.{name}: {instrs} instrs -> {ret}{}",
            if *has_loop { " [loop]" } else { "" }
        );
    }

    ExitCode::SUCCESS
}
