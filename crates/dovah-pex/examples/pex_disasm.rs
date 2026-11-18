//! Disassemble one `.pex` file to readable text.
//!
//! Usage: `cargo run -p dovah-pex --example pex_disasm --
//! <file.pex> [function-name-filter]`
//!
//! Prints the file header, each object's variables/properties, and
//! every function with its instructions numbered, with source line
//! numbers from the debug info when present. Built for the M7.1
//! real-script proving pass (design doc §7.8).

use dovah_pex::{Function, PexFile, Value};
use std::env;
use std::process::ExitCode;

fn fmt_value(value: &Value) -> String {
    match value {
        Value::None => "none".to_string(),
        Value::Identifier(name) => name.clone(),
        Value::Str(text) => format!("\"{text}\""),
        Value::Int(n) => n.to_string(),
        Value::Float(f) => format!("{f}"),
        Value::Bool(b) => b.to_string(),
    }
}

fn fmt_function(
    file: &PexFile,
    object_name: &str,
    state_name: &str,
    name: &str,
    function: &Function,
    filter: Option<&str>,
) {
    if let Some(want) = filter {
        if !name.eq_ignore_ascii_case(want) {
            return;
        }
    }
    let params: Vec<String> = function
        .params
        .iter()
        .map(|p| format!("{}: {}", p.name, p.type_name))
        .collect();
    let locals: Vec<String> = function
        .locals
        .iter()
        .map(|p| format!("{}: {}", p.name, p.type_name))
        .collect();
    let mut flags = Vec::new();
    if function.is_global() {
        flags.push("global");
    }
    if function.is_native() {
        flags.push("native");
    }
    println!(
        "\n  {name}({}) -> {}{}",
        params.join(", "),
        function.return_type,
        if flags.is_empty() {
            String::new()
        } else {
            format!(" [{}]", flags.join(", "))
        }
    );
    if !locals.is_empty() {
        println!("    locals: {}", locals.join(", "));
    }
    if function.is_native() {
        println!("    <native: no bytecode>");
        return;
    }
    // Source line numbers for this function, if the debug info has them.
    let lines: Option<&[u16]> = file.debug_info.as_ref().and_then(|d| {
        d.functions
            .iter()
            .find(|f| f.object == object_name && f.state == state_name && f.function == name)
            .map(|f| f.line_numbers.as_slice())
    });
    for (pc, instruction) in function.instructions.iter().enumerate() {
        let mnemonic = instruction.name().unwrap_or("???");
        let args: Vec<String> = instruction.args.iter().map(fmt_value).collect();
        let line = lines
            .and_then(|l| l.get(pc))
            .map(|n| format!("  ; line {n}"))
            .unwrap_or_default();
        println!("    {pc:4}  {mnemonic:<22} {}{line}", args.join(", "));
    }
}

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: pex_disasm <file.pex> [function-name-filter]");
        return ExitCode::from(2);
    };
    let filter = args.next();
    let file = match PexFile::open(&path) {
        Ok(file) => file,
        Err(e) => {
            eprintln!("{path}: {e}");
            return ExitCode::from(1);
        }
    };
    println!("{path}");
    println!(
        "version {}.{}, game id {}, source '{}'",
        file.major_version, file.minor_version, file.game_id, file.source_file_name
    );
    println!(
        "strings: {}, debug info: {}, user flags: {}, objects: {}",
        file.strings.len(),
        file.debug_info.is_some(),
        file.user_flags.len(),
        file.objects.len()
    );
    for object in &file.objects {
        println!(
            "\nscript {} extends {} (auto state '{}')",
            object.name, object.parent, object.auto_state
        );
        if !object.variables.is_empty() {
            println!("  variables:");
            for var in &object.variables {
                println!(
                    "    {}: {} = {}",
                    var.name,
                    var.type_name,
                    fmt_value(&var.default)
                );
            }
        }
        for property in &object.properties {
            let kind = if property.is_auto() {
                format!("auto ({})", property.auto_var.as_deref().unwrap_or("?"))
            } else {
                "full".to_string()
            };
            println!(
                "  property {}: {} [{}, read={}, write={}]",
                property.name,
                property.type_name,
                kind,
                property.can_read(),
                property.can_write()
            );
            if let Some(handler) = &property.read_handler {
                fmt_function(
                    &file,
                    &object.name,
                    "",
                    &format!("{}.get", property.name),
                    handler,
                    filter.as_deref(),
                );
            }
            if let Some(handler) = &property.write_handler {
                fmt_function(
                    &file,
                    &object.name,
                    "",
                    &format!("{}.set", property.name),
                    handler,
                    filter.as_deref(),
                );
            }
        }
        for state in &object.states {
            let label = if state.is_empty_state() {
                "<empty>".to_string()
            } else {
                state.name.clone()
            };
            println!("  state {label} ({} functions)", state.functions.len());
            for named in &state.functions {
                fmt_function(
                    &file,
                    &object.name,
                    &state.name,
                    &named.name,
                    &named.function,
                    filter.as_deref(),
                );
            }
        }
    }
    ExitCode::SUCCESS
}
