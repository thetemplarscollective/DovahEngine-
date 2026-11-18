//! Native functions: the engine-implemented side of Papyrus.
//!
//! Functions flagged `native` in a PEX carry no bytecode — the
//! engine implements them. The registry maps (script, function) to
//! a Rust function. A call to a native the engine has not
//! implemented is a runtime error that names the missing function,
//! never a silent no-op (SKSE natives fail this way too, by design).
//!
//! **Latent natives** (`Utility.Wait`, `WaitMenuMode`) do not
//! return immediately: they park the calling stack and resume it
//! later. A native signals that by returning
//! [`NativeOutcome::Park`]; the scheduler handles the rest.
//!
//! M7 registered a minimal set; M7.1 corrected it against the
//! real native declarations (the `.pex` files declare every
//! native with its param count) and the real call frequencies
//! (the `pex_sweep` example). Three M7 registrations were wrong
//! and are fixed here: `Debug.Trace` takes 2 arguments (text,
//! severity), not 1; the random functions are `Utility.RandomInt`
//! / `Utility.RandomFloat` (not `GetRandomInt` / `GetRandomFloat`)
//! and `RandomFloat` takes a (min, max) range; and `Math.Log`
//! does not exist (it is not declared in `Math.pex`) and was
//! removed. The set grows alongside gameplay, driven by what
//! real scripts call.

use crate::error::RuntimeError;
use crate::instance::InstanceId;
use crate::value::RuntimeValue;
use crate::vm::Vm;
use std::collections::HashMap;
use std::rc::Rc;

/// What a native call returns: either a value right now, or a
/// request to park the calling stack.
#[derive(Debug, Clone, PartialEq)]
pub enum NativeOutcome {
    /// The call completed; deliver this value to the caller.
    Value(RuntimeValue),
    /// Park the calling stack for `secs` game seconds, then resume
    /// it, delivering `resume` as the call's return value.
    Park { secs: f64, resume: RuntimeValue },
}

/// A native function implementation. It receives the VM (so it can
/// log, queue events, or read game state), the calling instance
/// (`None` when the caller is a global function, which has no
/// `Self`), and the call arguments.
pub type NativeFn =
    fn(&mut Vm, Option<InstanceId>, Vec<RuntimeValue>) -> Result<NativeOutcome, RuntimeError>;

/// One registered native: its arity (checked by the VM before the
/// call) and whether it is latent.
#[derive(Clone)]
pub struct NativeDef {
    pub script: String,
    pub name: String,
    pub arity: usize,
    pub latent: bool,
    pub func: NativeFn,
}

impl std::fmt::Debug for NativeDef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeDef")
            .field("script", &self.script)
            .field("name", &self.name)
            .field("arity", &self.arity)
            .field("latent", &self.latent)
            .finish_non_exhaustive()
    }
}

/// The native registry, keyed by (lowercased script, lowercased
/// function) — Papyrus names are case-insensitive.
#[derive(Default)]
pub struct NativeRegistry {
    natives: HashMap<(String, String), NativeDef>,
}

impl NativeRegistry {
    pub fn new() -> Self {
        NativeRegistry::default()
    }

    /// Register one native.
    pub fn register(
        &mut self,
        script: &str,
        name: &str,
        arity: usize,
        latent: bool,
        func: NativeFn,
    ) {
        self.natives.insert(
            (script.to_ascii_lowercase(), name.to_ascii_lowercase()),
            NativeDef {
                script: script.to_string(),
                name: name.to_string(),
                arity,
                latent,
                func,
            },
        );
    }

    /// Look up a native by (script, function), case-insensitively.
    pub fn get(&self, script: &str, name: &str) -> Option<&NativeDef> {
        self.natives
            .get(&(script.to_ascii_lowercase(), name.to_ascii_lowercase()))
    }

    pub fn len(&self) -> usize {
        self.natives.len()
    }

    pub fn is_empty(&self) -> bool {
        self.natives.is_empty()
    }

    /// Register the built-in set (see the module docs). Arities
    /// match the real declarations in the vanilla `.pex` files.
    pub fn register_defaults(&mut self) {
        self.register("Debug", "Trace", 2, false, debug_trace);
        self.register("Debug", "Notification", 1, false, debug_notification);
        self.register("Utility", "Wait", 1, true, utility_wait);
        self.register("Utility", "WaitMenuMode", 1, true, utility_wait);
        self.register("Utility", "WaitGameTime", 1, true, utility_wait);
        self.register("Utility", "RandomInt", 2, false, utility_random_int);
        self.register("Utility", "RandomFloat", 2, false, utility_random_float);
        self.register(
            "Utility",
            "GetCurrentGameTime",
            0,
            false,
            utility_get_current_game_time,
        );
        self.register(
            "Utility",
            "GetCurrentRealTime",
            0,
            false,
            utility_get_current_real_time,
        );
        self.register("Utility", "IsInMenuMode", 0, false, utility_is_in_menu_mode);
        self.register("Game", "GetPlayer", 0, false, game_get_player);
        self.register("Math", "Abs", 1, false, math_abs);
        self.register("Math", "Sqrt", 1, false, math_sqrt);
        self.register("Math", "Floor", 1, false, math_floor);
        self.register("Math", "Ceiling", 1, false, math_ceiling);
        self.register("Math", "Pow", 2, false, math_pow);
        self.register("Math", "Sin", 1, false, math_sin);
        self.register("Math", "Cos", 1, false, math_cos);
        self.register("Math", "Tan", 1, false, math_tan);
        self.register("Math", "Acos", 1, false, math_acos);
        self.register("Math", "Asin", 1, false, math_asin);
        self.register("Math", "Atan", 1, false, math_atan);
        self.register(
            "Math",
            "DegreesToRadians",
            1,
            false,
            math_degrees_to_radians,
        );
        self.register(
            "Math",
            "RadiansToDegrees",
            1,
            false,
            math_radians_to_degrees,
        );
        self.register("StringUtil", "Length", 1, false, string_length);
        self.register("StringUtil", "Substring", 3, false, string_substring);
        self.register("Script", "GotoState", 1, false, script_goto_state);
        self.register("Script", "GetState", 0, false, script_get_state);
        self.register(
            "Script",
            "RegisterForUpdate",
            1,
            false,
            script_register_for_update,
        );
        self.register(
            "Script",
            "RegisterForSingleUpdate",
            1,
            false,
            script_register_for_single_update,
        );
        self.register(
            "Script",
            "UnregisterForUpdate",
            0,
            false,
            script_unregister_for_update,
        );
    }
}

fn native_error(script: &str, function: &str, message: &str) -> RuntimeError {
    RuntimeError::Native {
        script: script.to_string(),
        function: function.to_string(),
        message: message.to_string(),
    }
}

/// An int argument, coercing floats by truncation (the compiler
/// does this implicitly for int parameters).
fn arg_int(
    args: &[RuntimeValue],
    index: usize,
    script: &str,
    function: &str,
) -> Result<i32, RuntimeError> {
    match &args[index] {
        RuntimeValue::Int(n) => Ok(*n),
        RuntimeValue::Float(f) => Ok(*f as i32),
        other => Err(native_error(
            script,
            function,
            &format!(
                "argument {} must be Int, got {}",
                index + 1,
                other.type_name()
            ),
        )),
    }
}

/// A float argument, coercing ints numerically.
fn arg_float(
    args: &[RuntimeValue],
    index: usize,
    script: &str,
    function: &str,
) -> Result<f32, RuntimeError> {
    match &args[index] {
        RuntimeValue::Float(f) => Ok(*f),
        RuntimeValue::Int(n) => Ok(*n as f32),
        other => Err(native_error(
            script,
            function,
            &format!(
                "argument {} must be Float, got {}",
                index + 1,
                other.type_name()
            ),
        )),
    }
}

fn arg_str(
    args: &[RuntimeValue],
    index: usize,
    script: &str,
    function: &str,
) -> Result<String, RuntimeError> {
    match &args[index] {
        RuntimeValue::Str(s) => Ok(s.to_string()),
        other => Err(native_error(
            script,
            function,
            &format!(
                "argument {} must be String, got {}",
                index + 1,
                other.type_name()
            ),
        )),
    }
}

fn value(outcome: RuntimeValue) -> Result<NativeOutcome, RuntimeError> {
    Ok(NativeOutcome::Value(outcome))
}

fn debug_trace(
    vm: &mut Vm,
    _caller: Option<InstanceId>,
    args: Vec<RuntimeValue>,
) -> Result<NativeOutcome, RuntimeError> {
    // Trace(text, severity): the severity (0 = info, 1 = warning,
    // 2 = error) is accepted and ignored — the trace log keeps
    // the message as-is.
    let message = arg_str(&args, 0, "Debug", "Trace")?;
    let _severity = arg_int(&args, 1, "Debug", "Trace")?;
    vm.trace_log.push(message);
    value(RuntimeValue::None)
}

/// `Debug.Notification`: show a short message to the player. There
/// is no HUD yet, so the message goes to the VM's notification
/// queue, which the HUD will read once it exists. The text is
/// preserved, not dropped.
fn debug_notification(
    vm: &mut Vm,
    _caller: Option<InstanceId>,
    args: Vec<RuntimeValue>,
) -> Result<NativeOutcome, RuntimeError> {
    let message = arg_str(&args, 0, "Debug", "Notification")?;
    vm.notifications.push(message);
    value(RuntimeValue::None)
}

/// `Utility.Wait` / `WaitMenuMode` / `WaitGameTime`: park the
/// stack for the given seconds, then resume with `None`. Menu
/// mode is not modelled yet, so `WaitMenuMode` behaves like
/// `Wait`. The VM's virtual clock *is* game time, so
/// `WaitGameTime` parks on it directly.
fn utility_wait(
    _vm: &mut Vm,
    _caller: Option<InstanceId>,
    args: Vec<RuntimeValue>,
) -> Result<NativeOutcome, RuntimeError> {
    let secs = arg_float(&args, 0, "Utility", "Wait")? as f64;
    Ok(NativeOutcome::Park {
        secs: if secs < 0.0 { 0.0 } else { secs },
        resume: RuntimeValue::None,
    })
}

fn utility_random_int(
    vm: &mut Vm,
    _caller: Option<InstanceId>,
    args: Vec<RuntimeValue>,
) -> Result<NativeOutcome, RuntimeError> {
    let mut min = arg_int(&args, 0, "Utility", "RandomInt")?;
    let mut max = arg_int(&args, 1, "Utility", "RandomInt")?;
    if min > max {
        std::mem::swap(&mut min, &mut max);
    }
    let span = (max as i64 - min as i64 + 1) as u64;
    let roll = vm.next_random_u64() % span;
    value(RuntimeValue::Int(min + roll as i32))
}

fn utility_random_float(
    vm: &mut Vm,
    _caller: Option<InstanceId>,
    args: Vec<RuntimeValue>,
) -> Result<NativeOutcome, RuntimeError> {
    // A float in [min, max) from the top 24 bits of the RNG.
    let mut min = arg_float(&args, 0, "Utility", "RandomFloat")?;
    let mut max = arg_float(&args, 1, "Utility", "RandomFloat")?;
    if min > max {
        std::mem::swap(&mut min, &mut max);
    }
    let roll = (vm.next_random_u64() >> 40) as f32 / (1u32 << 24) as f32;
    value(RuntimeValue::Float(min + roll * (max - min)))
}

/// `Utility.GetCurrentGameTime`: game time in days since the VM
/// started (game time 0 = the start of the game). The VM's
/// virtual clock starts at 0 and advances by `dt` each tick, so
/// this is `time / 86400`. Save/load must persist `vm.time` for
/// this to survive a reload (future work).
fn utility_get_current_game_time(
    vm: &mut Vm,
    _caller: Option<InstanceId>,
    _args: Vec<RuntimeValue>,
) -> Result<NativeOutcome, RuntimeError> {
    value(RuntimeValue::Float((vm.time / 86_400.0) as f32))
}

/// `Utility.GetCurrentRealTime`: real seconds since the VM was
/// created (the game launch, in engine terms).
fn utility_get_current_real_time(
    vm: &mut Vm,
    _caller: Option<InstanceId>,
    _args: Vec<RuntimeValue>,
) -> Result<NativeOutcome, RuntimeError> {
    value(RuntimeValue::Float(vm.real_elapsed_secs() as f32))
}

/// `Utility.IsInMenuMode`: whether a game menu is open. The
/// engine has no menus yet, so this is always false — truthfully,
/// not as a stub: there is no menu mode to be in.
fn utility_is_in_menu_mode(
    _vm: &mut Vm,
    _caller: Option<InstanceId>,
    _args: Vec<RuntimeValue>,
) -> Result<NativeOutcome, RuntimeError> {
    value(RuntimeValue::Bool(false))
}

fn game_get_player(
    vm: &mut Vm,
    _caller: Option<InstanceId>,
    _args: Vec<RuntimeValue>,
) -> Result<NativeOutcome, RuntimeError> {
    value(match vm.player {
        Some(id) => RuntimeValue::Object(id),
        None => RuntimeValue::None,
    })
}

fn math_abs(
    _vm: &mut Vm,
    _caller: Option<InstanceId>,
    args: Vec<RuntimeValue>,
) -> Result<NativeOutcome, RuntimeError> {
    match &args[0] {
        RuntimeValue::Int(n) => value(RuntimeValue::Int(n.abs())),
        RuntimeValue::Float(f) => value(RuntimeValue::Float(f.abs())),
        other => Err(native_error(
            "Math",
            "Abs",
            &format!("argument 1 must be Int or Float, got {}", other.type_name()),
        )),
    }
}

fn math_unary_float(
    args: &[RuntimeValue],
    name: &str,
    f: fn(f32) -> f32,
) -> Result<NativeOutcome, RuntimeError> {
    let x = arg_float(args, 0, "Math", name)?;
    value(RuntimeValue::Float(f(x)))
}

fn math_sqrt(
    _vm: &mut Vm,
    _caller: Option<InstanceId>,
    args: Vec<RuntimeValue>,
) -> Result<NativeOutcome, RuntimeError> {
    math_unary_float(&args, "Sqrt", f32::sqrt)
}

fn math_sin(
    _vm: &mut Vm,
    _caller: Option<InstanceId>,
    args: Vec<RuntimeValue>,
) -> Result<NativeOutcome, RuntimeError> {
    math_unary_float(&args, "Sin", f32::sin)
}

fn math_cos(
    _vm: &mut Vm,
    _caller: Option<InstanceId>,
    args: Vec<RuntimeValue>,
) -> Result<NativeOutcome, RuntimeError> {
    math_unary_float(&args, "Cos", f32::cos)
}

fn math_tan(
    _vm: &mut Vm,
    _caller: Option<InstanceId>,
    args: Vec<RuntimeValue>,
) -> Result<NativeOutcome, RuntimeError> {
    math_unary_float(&args, "Tan", f32::tan)
}

fn math_floor(
    _vm: &mut Vm,
    _caller: Option<InstanceId>,
    args: Vec<RuntimeValue>,
) -> Result<NativeOutcome, RuntimeError> {
    let x = arg_float(&args, 0, "Math", "Floor")?;
    value(RuntimeValue::Int(x.floor() as i32))
}

fn math_ceiling(
    _vm: &mut Vm,
    _caller: Option<InstanceId>,
    args: Vec<RuntimeValue>,
) -> Result<NativeOutcome, RuntimeError> {
    let x = arg_float(&args, 0, "Math", "Ceiling")?;
    value(RuntimeValue::Int(x.ceil() as i32))
}

fn math_pow(
    _vm: &mut Vm,
    _caller: Option<InstanceId>,
    args: Vec<RuntimeValue>,
) -> Result<NativeOutcome, RuntimeError> {
    let base = arg_float(&args, 0, "Math", "Pow")?;
    let exp = arg_float(&args, 1, "Math", "Pow")?;
    value(RuntimeValue::Float(base.powf(exp)))
}

fn math_acos(
    _vm: &mut Vm,
    _caller: Option<InstanceId>,
    args: Vec<RuntimeValue>,
) -> Result<NativeOutcome, RuntimeError> {
    math_unary_float(&args, "Acos", f32::acos)
}

fn math_asin(
    _vm: &mut Vm,
    _caller: Option<InstanceId>,
    args: Vec<RuntimeValue>,
) -> Result<NativeOutcome, RuntimeError> {
    math_unary_float(&args, "Asin", f32::asin)
}

fn math_atan(
    _vm: &mut Vm,
    _caller: Option<InstanceId>,
    args: Vec<RuntimeValue>,
) -> Result<NativeOutcome, RuntimeError> {
    math_unary_float(&args, "Atan", f32::atan)
}

fn math_degrees_to_radians(
    _vm: &mut Vm,
    _caller: Option<InstanceId>,
    args: Vec<RuntimeValue>,
) -> Result<NativeOutcome, RuntimeError> {
    math_unary_float(&args, "DegreesToRadians", f32::to_radians)
}

fn math_radians_to_degrees(
    _vm: &mut Vm,
    _caller: Option<InstanceId>,
    args: Vec<RuntimeValue>,
) -> Result<NativeOutcome, RuntimeError> {
    math_unary_float(&args, "RadiansToDegrees", f32::to_degrees)
}

fn string_length(
    _vm: &mut Vm,
    _caller: Option<InstanceId>,
    args: Vec<RuntimeValue>,
) -> Result<NativeOutcome, RuntimeError> {
    let s = arg_str(&args, 0, "StringUtil", "Length")?;
    value(RuntimeValue::Int(s.chars().count() as i32))
}

fn string_substring(
    _vm: &mut Vm,
    _caller: Option<InstanceId>,
    args: Vec<RuntimeValue>,
) -> Result<NativeOutcome, RuntimeError> {
    let s = arg_str(&args, 0, "StringUtil", "Substring")?;
    let start = arg_int(&args, 1, "StringUtil", "Substring")?;
    let length = arg_int(&args, 2, "StringUtil", "Substring")?;
    let chars: Vec<char> = s.chars().collect();
    let start = start.max(0) as usize;
    let length = length.max(0) as usize;
    let end = (start + length).min(chars.len());
    let out: String = if start >= chars.len() {
        String::new()
    } else {
        chars[start..end].iter().collect()
    };
    value(RuntimeValue::Str(Rc::from(out.as_str())))
}

fn caller_or_error(caller: Option<InstanceId>, function: &str) -> Result<InstanceId, RuntimeError> {
    caller.ok_or_else(|| {
        native_error(
            "Script",
            function,
            "called from a global function, which has no Self",
        )
    })
}

fn script_goto_state(
    vm: &mut Vm,
    caller: Option<InstanceId>,
    args: Vec<RuntimeValue>,
) -> Result<NativeOutcome, RuntimeError> {
    let id = caller_or_error(caller, "GotoState")?;
    let new_state = arg_str(&args, 0, "Script", "GotoState")?;
    vm.goto_state(id, &new_state)?;
    value(RuntimeValue::None)
}

fn script_get_state(
    vm: &mut Vm,
    caller: Option<InstanceId>,
    _args: Vec<RuntimeValue>,
) -> Result<NativeOutcome, RuntimeError> {
    let id = caller_or_error(caller, "GetState")?;
    let state = vm
        .instances
        .get(id)
        .map(|i| i.state.clone())
        .unwrap_or_default();
    let interned = vm.interner.intern(&state);
    value(RuntimeValue::Str(interned))
}

fn script_register_for_update(
    vm: &mut Vm,
    caller: Option<InstanceId>,
    args: Vec<RuntimeValue>,
) -> Result<NativeOutcome, RuntimeError> {
    let id = caller_or_error(caller, "RegisterForUpdate")?;
    let interval = arg_float(&args, 0, "Script", "RegisterForUpdate")? as f64;
    vm.register_for_update(id, interval, true);
    value(RuntimeValue::None)
}

fn script_register_for_single_update(
    vm: &mut Vm,
    caller: Option<InstanceId>,
    args: Vec<RuntimeValue>,
) -> Result<NativeOutcome, RuntimeError> {
    let id = caller_or_error(caller, "RegisterForSingleUpdate")?;
    let interval = arg_float(&args, 0, "Script", "RegisterForSingleUpdate")? as f64;
    vm.register_for_update(id, interval, false);
    value(RuntimeValue::None)
}

fn script_unregister_for_update(
    vm: &mut Vm,
    caller: Option<InstanceId>,
    _args: Vec<RuntimeValue>,
) -> Result<NativeOutcome, RuntimeError> {
    let id = caller_or_error(caller, "UnregisterForUpdate")?;
    vm.unregister_for_update(id);
    value(RuntimeValue::None)
}
