//! The Papyrus virtual machine: frames, the interpreter loop, the
//! event queue, and the time-sliced scheduler.
//!
//! Papyrus bytecode is three-address code — arithmetic writes to a
//! named destination, calls carry their arguments in the
//! instruction — so there is no operand stack. A running script is a
//! **stack** of frames, and the VM runs many stacks: each game tick
//! it spends a small instruction budget across the runnable queue,
//! parks stacks that call latent natives, and fires update timers.
//! A script stuck in a loop consumes its tick budget and yields, so
//! it cannot freeze the renderer.
//!
//! The two undocumented bytecode points (jump targets, call operand
//! order) live in [`crate::semantics`]; the interpreter consults
//! that module in exactly two places and nowhere else.

use crate::error::RuntimeError;
use crate::instance::{InstanceId, InstanceStore};
use crate::native::{NativeOutcome, NativeRegistry};
use crate::resolve::{
    resolve_function, resolve_parent_function, resolve_property, resolve_variable, ScriptRegistry,
};
use crate::semantics::Semantics;
use crate::value::{CastContext, RuntimeValue, StringInterner};
use dovah_esm::vmad::{Vmad, VmadProperty, VmadValue};
use dovah_pex::{Function, PexFile, PexObject, Value};
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::time::Instant;

/// One function call in progress.
#[derive(Debug)]
pub struct Frame {
    /// The function being executed (an owned clone of the parsed
    /// bytecode — Papyrus functions are small).
    pub function: Function,
    /// The script that defined this function.
    pub script: String,
    /// The state the function was resolved in.
    pub state: String,
    /// The function name, for error messages.
    pub name: String,
    /// `Self`: the instance the function runs on. `None` for global
    /// functions, which have no `Self`.
    pub instance: Option<InstanceId>,
    /// Instruction pointer: index into `function.instructions`.
    pub pc: usize,
    /// Parameters and locals by name.
    pub locals: HashMap<String, RuntimeValue>,
    /// Where the return value goes in the *caller* frame (`None`
    /// when the caller discards it, or when this is the bottom
    /// frame of an event stack).
    pub return_dest: Option<String>,
}

/// Identifies a script function to call: its script, the state
/// it resolves in, and its name.
#[derive(Debug, Clone, Copy)]
struct CallTarget<'a> {
    script: &'a str,
    state: &'a str,
    name: &'a str,
}

/// Identifies a native function: its script and name (natives are
/// stateless).
#[derive(Debug, Clone, Copy)]
struct NativeTarget<'a> {
    script: &'a str,
    name: &'a str,
}

/// A running script: a stack of frames, started by one event (or
/// one property-set call).
#[derive(Debug)]
pub struct Stack {
    pub id: u64,
    /// What started the stack, e.g. `"OnInit"` — for diagnostics.
    pub event: String,
    /// The instance the event was posted to.
    pub instance: InstanceId,
    pub frames: Vec<Frame>,
}

/// What running a stack for one budget slice produced.
#[derive(Debug)]
pub enum StackOutcome {
    /// The stack ran to completion (its last frame returned).
    Finished,
    /// The budget ran out mid-stack; the stack is returned to the
    /// runnable queue to resume next tick.
    Yielded(Stack),
    /// A latent native parked the stack: resume it after `secs`
    /// game seconds, delivering `resume` to `dest`.
    Parked {
        stack: Stack,
        secs: f64,
        resume: RuntimeValue,
        dest: Option<String>,
    },
    /// The stack hit a runtime error and was dropped.
    Error(RuntimeError),
}

/// A parked stack, waiting for its wake time.
#[derive(Debug)]
struct ParkedStack {
    wake_time: f64,
    stack: Stack,
    resume_dest: Option<String>,
    resume_value: RuntimeValue,
}

/// A `RegisterForUpdate` timer: fires `OnUpdate` on the instance.
#[derive(Debug)]
struct UpdateTimer {
    instance: InstanceId,
    interval: f64,
    next_fire: f64,
    repeat: bool,
}

/// The Papyrus virtual machine.
pub struct Vm {
    /// Loaded script definitions.
    pub scripts: ScriptRegistry,
    /// Live script instances.
    pub instances: InstanceStore,
    /// Engine-implemented functions.
    pub natives: NativeRegistry,
    /// The reading of the two undocumented bytecode points.
    pub semantics: Semantics,
    /// The string pool.
    pub interner: StringInterner,
    /// Stacks waiting to run, in posting order.
    pub runnable: VecDeque<Stack>,
    /// Stacks parked by latent natives.
    parked: Vec<ParkedStack>,
    /// `RegisterForUpdate` timers.
    timers: Vec<UpdateTimer>,
    /// Virtual game time in seconds, advanced by `run_tick`.
    pub time: f64,
    /// Everything scripts printed with `Debug.Trace`, in order.
    pub trace_log: Vec<String>,
    /// Messages scripts sent with `Debug.Notification`, in order.
    /// The HUD reads these once it exists.
    pub notifications: Vec<String>,
    /// The return value of the most recently finished stack's
    /// bottom frame. `call_function` clears this when it queues
    /// the call, so after `run_until_idle` it holds that call's
    /// return value (`None` when the call errored or returned
    /// nothing).
    pub last_stack_result: Option<RuntimeValue>,
    /// Wall-clock time the VM was created at, for
    /// `Utility.GetCurrentRealTime`.
    started_at: Instant,
    /// Runtime errors from dropped stacks, in order.
    pub errors: Vec<RuntimeError>,
    /// Non-fatal problems (missing scripts at attach time, VMAD
    /// values that could not be applied, unresolved quest aliases).
    pub warnings: Vec<String>,
    /// The player instance, for `Game.GetPlayer` (`None` until the
    /// engine sets it).
    pub player: Option<InstanceId>,
    /// Instructions executed across all stacks (for the report).
    pub instructions_executed: u64,
    /// Stacks run to completion.
    pub stacks_finished: u64,
    /// VMAD object properties that point at quest aliases. Alias
    /// resolution needs the quest's alias table (QUST fields the
    /// ESM reader does not parse yet), so these are recorded and
    /// set to `None` for now. Entries are (instance, property,
    /// quest form ID, alias ID).
    pub unresolved_aliases: Vec<(InstanceId, String, u32, u16)>,
    next_stack_id: u64,
    rng_state: u64,
}

impl Vm {
    pub fn new() -> Self {
        let mut natives = NativeRegistry::new();
        natives.register_defaults();
        Vm {
            scripts: ScriptRegistry::new(),
            instances: InstanceStore::new(),
            natives,
            semantics: Semantics,
            interner: StringInterner::new(),
            runnable: VecDeque::new(),
            parked: Vec::new(),
            timers: Vec::new(),
            time: 0.0,
            trace_log: Vec::new(),
            notifications: Vec::new(),
            last_stack_result: None,
            started_at: Instant::now(),
            errors: Vec::new(),
            warnings: Vec::new(),
            player: None,
            instructions_executed: 0,
            stacks_finished: 0,
            unresolved_aliases: Vec::new(),
            next_stack_id: 0,
            // Fixed seed: script behavior is deterministic until
            // the engine seeds from the clock.
            rng_state: 0x9E37_79B9_7F4A_7C15,
        }
    }

    /// Load every script definition in a parsed `.pex` file.
    pub fn load_pex(&mut self, file: &PexFile) {
        self.scripts.add_file(file);
    }

    /// Load one script definition.
    pub fn load_object(&mut self, object: PexObject) {
        self.scripts.add_object(object);
    }

    /// Set the player instance (`Game.GetPlayer` returns it).
    pub fn set_player(&mut self, id: InstanceId) {
        self.player = Some(id);
    }

    /// Real seconds since the VM was created.
    pub fn real_elapsed_secs(&self) -> f64 {
        self.started_at.elapsed().as_secs_f64()
    }

    /// Seed the deterministic RNG (used by `Utility.RandomInt` /
    /// `RandomFloat`).
    pub fn seed_rng(&mut self, seed: u64) {
        self.rng_state = if seed == 0 { 1 } else { seed };
    }

    /// The next 64-bit value from the VM's xorshift RNG.
    pub fn next_random_u64(&mut self) -> u64 {
        // xorshift64star.
        let mut x = self.rng_state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng_state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    // ---- instances -------------------------------------------------

    /// Create the instance of `script_name` on `form_id`, with its
    /// variables at their PEX defaults, its state at the script's
    /// auto state, and — when `vmad` is given — its properties
    /// initialized from the VMAD values. Queues the instance's
    /// `OnInit` event. Idempotent: returns the existing instance
    /// when one already exists for this (form, script).
    ///
    /// VMAD values that cannot be applied (a property the script
    /// does not define, a value of the wrong type) become warnings,
    /// not errors — a bad property must not kill the instance.
    pub fn create_instance(
        &mut self,
        form_id: u32,
        script_name: &str,
        vmad: Option<&Vmad>,
    ) -> Result<InstanceId, RuntimeError> {
        if let Some(id) = self.instances.find(form_id, script_name) {
            return Ok(id);
        }
        let chain = self.scripts.chain(script_name);
        if chain.is_empty() {
            return Err(RuntimeError::MissingScript {
                script: script_name.to_string(),
            });
        }
        let id = self.instances.create(&chain, form_id, &mut self.interner);
        if let Some(vmad) = vmad {
            if let Some(vmad_script) = vmad
                .scripts
                .iter()
                .find(|s| s.name.eq_ignore_ascii_case(script_name))
            {
                if vmad_script.status != 3 {
                    for property in &vmad_script.properties {
                        if property.status == 3 {
                            continue;
                        }
                        if let Err(e) = self.apply_vmad_property(id, script_name, property) {
                            self.warnings.push(format!(
                                "form 0x{form_id:08x} {script_name}.{}: {e}",
                                property.name
                            ));
                        }
                    }
                }
            }
        }
        self.post_event(id, "OnInit", vec![]);
        Ok(id)
    }

    /// Attach every script in a record's VMAD field to the form:
    /// create an instance per script (status 3, "inherited then
    /// removed", is skipped) and return the created instance IDs.
    /// Scripts whose definitions are not loaded become warnings.
    pub fn attach_scripts_from_vmad(&mut self, form_id: u32, vmad: &Vmad) -> Vec<InstanceId> {
        let mut created = Vec::new();
        for script in &vmad.scripts {
            if script.status == 3 {
                continue;
            }
            match self.create_instance(form_id, &script.name, Some(vmad)) {
                Ok(id) => created.push(id),
                Err(RuntimeError::MissingScript { script }) => {
                    self.warnings.push(format!(
                        "script '{script}' attached to form 0x{form_id:08x} is not loaded"
                    ));
                }
                Err(e) => {
                    self.warnings.push(format!(
                        "form 0x{form_id:08x} script '{}': {e}",
                        script.name
                    ));
                }
            }
        }
        created
    }

    /// Find or create the instance of `type_name` on `form_id`:
    /// an existing instance of that script (or a script extending
    /// it) wins; otherwise a fresh instance is created with default
    /// values and no VMAD. Returns `None` when the script is not
    /// loaded. This is how an object-typed VMAD property resolves
    /// to an instance.
    pub fn instance_for(&mut self, form_id: u32, type_name: &str) -> Option<InstanceId> {
        for id in self.instances.on_form(form_id) {
            let script = self.instances.get(id)?.script.clone();
            if self.scripts.is_subtype(&script, type_name) {
                return Some(id);
            }
        }
        self.create_instance(form_id, type_name, None).ok()
    }

    /// Apply one VMAD initial property value to an instance. Auto
    /// properties set their backing variable directly; full
    /// properties are applied by calling the set handler (queued
    /// before `OnInit`, so the handler runs first).
    fn apply_vmad_property(
        &mut self,
        id: InstanceId,
        script_name: &str,
        property: &VmadProperty,
    ) -> Result<(), RuntimeError> {
        let (_, definition) = resolve_property(&self.scripts, script_name, &property.name)
            .ok_or_else(|| RuntimeError::MissingProperty {
                script: script_name.to_string(),
                property: property.name.clone(),
            })?;
        let type_name = definition.type_name.clone();
        let is_auto = definition.is_auto();
        let auto_var = definition.auto_var.clone();
        let value = self.vmad_value_to_runtime(id, &property.name, &property.value, &type_name)?;
        let value = self.coerce_for_type(value, &type_name)?;
        if is_auto {
            let var = auto_var.ok_or_else(|| RuntimeError::MissingProperty {
                script: script_name.to_string(),
                property: property.name.clone(),
            })?;
            if let Some(instance) = self.instances.get_mut(id) {
                instance.set(&var, value);
            }
            Ok(())
        } else {
            self.post_property_set(id, script_name, &property.name, value)
        }
    }

    /// Queue a call to a full property's set handler, with the
    /// given value as its argument. The stack is queued ahead of
    /// `OnInit` (callers push it first), so the handler runs before
    /// the script initializes.
    fn post_property_set(
        &mut self,
        id: InstanceId,
        script_name: &str,
        property_name: &str,
        value: RuntimeValue,
    ) -> Result<(), RuntimeError> {
        let (_, definition) = resolve_property(&self.scripts, script_name, property_name)
            .ok_or_else(|| RuntimeError::MissingProperty {
                script: script_name.to_string(),
                property: property_name.to_string(),
            })?;
        let Some(handler) = definition.write_handler.clone() else {
            return Err(RuntimeError::PropertyAccess {
                script: script_name.to_string(),
                property: property_name.to_string(),
            });
        };
        let frame = self.make_frame(
            CallTarget {
                script: script_name,
                state: "",
                name: &format!("{property_name} setter"),
            },
            handler,
            Some(id),
            vec![value],
            None,
        )?;
        self.push_stack(id, &format!("{property_name} setter"), vec![frame]);
        Ok(())
    }

    /// Convert a VMAD property value into a runtime value. Object
    /// values resolve to the matching instance on the referenced
    /// form (created if needed); quest-alias values cannot be
    /// resolved yet (see `unresolved_aliases`) and become `None`.
    fn vmad_value_to_runtime(
        &mut self,
        owner: InstanceId,
        property_name: &str,
        value: &VmadValue,
        type_name: &str,
    ) -> Result<RuntimeValue, RuntimeError> {
        Ok(match value {
            VmadValue::Object { form_id, alias_id } => {
                if let Some(alias) = alias_id {
                    self.unresolved_aliases.push((
                        owner,
                        property_name.to_string(),
                        *form_id,
                        *alias,
                    ));
                    RuntimeValue::None
                } else {
                    match self.instance_for(*form_id, type_name) {
                        Some(id) => RuntimeValue::Object(id),
                        None => RuntimeValue::None,
                    }
                }
            }
            VmadValue::Str(s) => RuntimeValue::Str(self.interner.intern(s)),
            VmadValue::Int(n) => RuntimeValue::Int(*n),
            VmadValue::Float(f) => RuntimeValue::Float(*f),
            VmadValue::Bool(b) => RuntimeValue::Bool(*b),
            VmadValue::Array(items) => {
                let element_type = type_name.strip_suffix("[]").unwrap_or("Var");
                let mut out = Vec::with_capacity(items.len());
                for item in items {
                    out.push(self.vmad_value_to_runtime(
                        owner,
                        property_name,
                        item,
                        element_type,
                    )?);
                }
                RuntimeValue::Array(Rc::new(RefCell::new(out)))
            }
        })
    }

    /// Coerce a value to its declared type when the kinds differ
    /// (e.g. a VMAD int for a Float property). Object and array
    /// values are already of the right kind and pass through.
    fn coerce_for_type(
        &mut self,
        value: RuntimeValue,
        type_name: &str,
    ) -> Result<RuntimeValue, RuntimeError> {
        let needs_cast = matches!(
            (&value, type_name),
            (RuntimeValue::Int(_), "Float")
                | (RuntimeValue::Float(_), "Int")
                | (RuntimeValue::Int(_), "String")
                | (RuntimeValue::Float(_), "String")
                | (RuntimeValue::Bool(_), "String")
                | (RuntimeValue::Str(_), "Int")
                | (RuntimeValue::Str(_), "Float")
        );
        if !needs_cast {
            return Ok(value);
        }
        value
            .cast_to(type_name, self)
            .map_err(|_| RuntimeError::BadCast {
                from: value.type_name().to_string(),
                to: type_name.to_string(),
            })
    }

    // ---- events and states -----------------------------------------

    /// Post an event to an instance: resolve the event function in
    /// the instance's current state and queue a new stack for it.
    /// Returns `false` (and queues nothing) when the script does
    /// not define the event — events are optional.
    pub fn post_event(
        &mut self,
        instance: InstanceId,
        name: &str,
        args: Vec<RuntimeValue>,
    ) -> bool {
        let Some(inst) = self.instances.get(instance) else {
            return false;
        };
        let (script, state) = (inst.script.clone(), inst.state.clone());
        let Some(resolved) = resolve_function(&self.scripts, &script, &state, name) else {
            return false;
        };
        let function = resolved.function.clone();
        let owner = resolved.owner.name.clone();
        if function.is_native() {
            // Events are never native in practice; if one resolves
            // to a native, run it now and ignore any latent park.
            let _ = self.call_native(&owner, name, Some(instance), args);
            return true;
        }
        match self.make_frame(
            CallTarget {
                script: &owner,
                state: &state,
                name,
            },
            function,
            Some(instance),
            args,
            None,
        ) {
            Ok(frame) => {
                self.push_stack(instance, name, vec![frame]);
                true
            }
            Err(e) => {
                self.errors.push(e);
                false
            }
        }
    }

    /// Call a script function on an instance by name, resolving
    /// it in the instance's current state like `post_event`, and
    /// read its return value back from `last_stack_result` once
    /// the stack finishes (run the VM until idle first). This is
    /// the engine's way to invoke script logic directly (quest
    /// functions, dialogue conditions, the M7.1 proving pass), as
    /// opposed to posting an event. Script functions win over
    /// natives, as in `invoke`; when no script function exists,
    /// the native registry is tried (own script, then the
    /// `Script` base). Returns `false` (and queues nothing) when
    /// the function does not exist at all. A call to a latent
    /// native through this path returns `false` and records an
    /// error: there is no stack to park.
    pub fn call_function(
        &mut self,
        instance: InstanceId,
        name: &str,
        args: Vec<RuntimeValue>,
    ) -> bool {
        let Some(inst) = self.instances.get(instance) else {
            return false;
        };
        let (script, state) = (inst.script.clone(), inst.state.clone());
        if let Some(resolved) = resolve_function(&self.scripts, &script, &state, name) {
            let function = resolved.function.clone();
            let owner = resolved.owner.name.clone();
            if function.is_native() {
                return self.call_function_native(&owner, name, Some(instance), args);
            }
            return match self.make_frame(
                CallTarget {
                    script: &owner,
                    state: &state,
                    name,
                },
                function,
                Some(instance),
                args,
                None,
            ) {
                Ok(frame) => {
                    self.last_stack_result = None;
                    self.push_stack(instance, name, vec![frame]);
                    true
                }
                Err(e) => {
                    self.errors.push(e);
                    false
                }
            };
        }
        // No script function: try the natives (own script, then
        // the Script base). A function that exists nowhere is a
        // plain `false`, like `post_event` — not an error.
        match self.call_native_or_script_base(&script, name, Some(instance), args, true) {
            Ok(NativeOutcome::Value(value)) => {
                self.last_stack_result = Some(value);
                true
            }
            Ok(NativeOutcome::Park { .. }) => {
                self.errors.push(RuntimeError::Native {
                    script,
                    function: name.to_string(),
                    message: "latent native cannot be called via call_function".to_string(),
                });
                false
            }
            Err(RuntimeError::MissingNative { .. }) => false,
            Err(e) => {
                self.errors.push(e);
                false
            }
        }
    }

    /// Run a resolved native immediately for `call_function`,
    /// storing its return value in `last_stack_result`.
    fn call_function_native(
        &mut self,
        script: &str,
        name: &str,
        instance: Option<InstanceId>,
        args: Vec<RuntimeValue>,
    ) -> bool {
        match self.call_native(script, name, instance, args) {
            Ok(NativeOutcome::Value(value)) => {
                self.last_stack_result = Some(value);
                true
            }
            Ok(NativeOutcome::Park { .. }) => {
                self.errors.push(RuntimeError::Native {
                    script: script.to_string(),
                    function: name.to_string(),
                    message: "latent native cannot be called via call_function".to_string(),
                });
                false
            }
            Err(e) => {
                self.errors.push(e);
                false
            }
        }
    }

    /// Change an instance's state, firing `OnEndState` in the old
    /// state and `OnBeginState` in the new one. A change to the
    /// state the instance is already in is a no-op.
    pub fn goto_state(&mut self, id: InstanceId, new_state: &str) -> Result<(), RuntimeError> {
        let Some(instance) = self.instances.get(id) else {
            return Err(RuntimeError::UnknownInstance { id });
        };
        if instance.state.eq_ignore_ascii_case(new_state) {
            return Ok(());
        }
        self.post_event(id, "OnEndState", vec![]);
        let interned = self.interner.intern(new_state);
        if let Some(instance) = self.instances.get_mut(id) {
            instance.state = new_state.to_string();
            // Keep the `::state` variable in sync (see `write_var`).
            instance.set("::state", RuntimeValue::Str(interned));
        }
        self.post_event(id, "OnBeginState", vec![]);
        Ok(())
    }

    /// Register (or re-register) an `OnUpdate` timer for an
    /// instance. A repeat timer fires every `interval` seconds; a
    /// single-update timer fires once.
    pub fn register_for_update(&mut self, instance: InstanceId, interval: f64, repeat: bool) {
        self.timers.retain(|t| t.instance != instance);
        self.timers.push(UpdateTimer {
            instance,
            interval,
            next_fire: self.time + interval,
            repeat,
        });
    }

    /// Cancel an instance's `OnUpdate` timer, if any.
    pub fn unregister_for_update(&mut self, instance: InstanceId) {
        self.timers.retain(|t| t.instance != instance);
    }

    // ---- scheduling ------------------------------------------------

    /// Run one game tick: advance virtual time by `dt`, wake parked
    /// stacks whose time has come, fire due update timers, then run
    /// runnable stacks until `budget` instructions are spent.
    pub fn run_tick(&mut self, budget: usize, dt: f64) {
        self.time += dt;
        let now = self.time;

        // Wake parked stacks, delivering the latent call's return
        // value to its destination variable.
        let mut index = 0;
        while index < self.parked.len() {
            if self.parked[index].wake_time <= now {
                let parked = self.parked.remove(index);
                let mut stack = parked.stack;
                if let Some(dest) = &parked.resume_dest {
                    let value = parked.resume_value.clone();
                    if let Err(e) = self.write_var(&mut stack, dest, value) {
                        self.errors.push(e);
                        continue;
                    }
                }
                self.runnable.push_back(stack);
            } else {
                index += 1;
            }
        }

        // Fire update timers.
        let mut fired: Vec<InstanceId> = Vec::new();
        for timer in self.timers.iter_mut() {
            if timer.next_fire <= now {
                fired.push(timer.instance);
                if timer.repeat {
                    timer.next_fire += timer.interval;
                } else {
                    timer.next_fire = f64::INFINITY;
                }
            }
        }
        self.timers.retain(|t| t.next_fire.is_finite());
        for id in fired {
            self.post_event(id, "OnUpdate", vec![]);
        }

        // Run stacks until the budget is spent.
        let mut remaining = budget;
        while remaining > 0 {
            let Some(stack) = self.runnable.pop_front() else {
                break;
            };
            let (outcome, used) = self.run_stack(stack, remaining);
            remaining -= used;
            match outcome {
                StackOutcome::Finished => {
                    self.stacks_finished += 1;
                }
                StackOutcome::Yielded(stack) => {
                    self.runnable.push_back(stack);
                }
                StackOutcome::Parked {
                    stack,
                    secs,
                    resume,
                    dest,
                } => {
                    self.parked.push(ParkedStack {
                        wake_time: now + secs,
                        stack,
                        resume_dest: dest,
                        resume_value: resume,
                    });
                }
                StackOutcome::Error(e) => {
                    self.errors.push(e);
                }
            }
        }
    }

    /// Run ticks (with no time passing) until the runnable queue is
    /// empty or `max_ticks` ticks have run. Returns `true` when the
    /// queue drained. A test helper — the game calls `run_tick`
    /// once per frame instead.
    pub fn run_until_idle(&mut self, budget_per_tick: usize, max_ticks: usize) -> bool {
        for _ in 0..max_ticks {
            if self.runnable.is_empty() {
                return true;
            }
            self.run_tick(budget_per_tick, 0.0);
        }
        self.runnable.is_empty()
    }

    /// Stacks waiting to run plus stacks parked on latent calls.
    pub fn pending_stacks(&self) -> usize {
        self.runnable.len() + self.parked.len()
    }

    /// Drop every runnable stack without running it. The engine
    /// uses this when unloading (scripts on unloaded forms stop),
    /// and the M7.1 proving pass uses it to drop a stack that
    /// never finishes its budget so one stuck script cannot hold
    /// up the rest of the census. Parked stacks and timers are
    /// untouched.
    pub fn drop_runnable_stacks(&mut self) {
        self.runnable.clear();
    }

    fn push_stack(&mut self, instance: InstanceId, event: &str, frames: Vec<Frame>) {
        let id = self.next_stack_id;
        self.next_stack_id += 1;
        self.runnable.push_back(Stack {
            id,
            event: event.to_string(),
            instance,
            frames,
        });
    }

    // ---- the interpreter -------------------------------------------

    /// Run one stack until it finishes, parks, errors, or spends
    /// `budget` instructions. Returns the outcome and the number of
    /// instructions executed.
    fn run_stack(&mut self, mut stack: Stack, budget: usize) -> (StackOutcome, usize) {
        let mut used = 0;
        while used < budget {
            if stack.frames.is_empty() {
                return (StackOutcome::Finished, used);
            }
            match self.step(&mut stack) {
                Ok(Step::Continue) => {
                    used += 1;
                    self.instructions_executed += 1;
                }
                Ok(Step::Park { secs, resume, dest }) => {
                    used += 1;
                    self.instructions_executed += 1;
                    return (
                        StackOutcome::Parked {
                            stack,
                            secs,
                            resume,
                            dest,
                        },
                        used,
                    );
                }
                Err(e) => {
                    return (StackOutcome::Error(e), used);
                }
            }
        }
        (StackOutcome::Yielded(stack), used)
    }

    /// Execute one instruction of the top frame.
    fn step(&mut self, stack: &mut Stack) -> Result<Step, RuntimeError> {
        // Fetch the instruction, then advance past it by default;
        // jumps overwrite `pc` below.
        let instruction = {
            let frame = stack.frames.last_mut().expect("step with no frame");
            if frame.pc >= frame.function.instructions.len() {
                // Fell off the end: an implicit `return None`.
                return self.return_from_frame(stack, RuntimeValue::None);
            }
            let instruction = frame.function.instructions[frame.pc].clone();
            frame.pc += 1;
            instruction
        };
        self.execute(stack, &instruction)
    }

    /// Pop the top frame, delivering `value` to the caller frame's
    /// return destination (if any).
    fn return_from_frame(
        &mut self,
        stack: &mut Stack,
        value: RuntimeValue,
    ) -> Result<Step, RuntimeError> {
        let frame = stack.frames.pop().expect("return with no frame");
        let bottom = stack.frames.is_empty();
        if bottom {
            // The bottom frame returned: record the value for
            // `call_function` callers.
            self.last_stack_result = Some(value.clone());
        }
        if let Some(dest) = frame.return_dest {
            if !bottom {
                self.write_var(stack, &dest, value)?;
            }
        }
        Ok(Step::Continue)
    }

    /// Build a frame for a function call: bind arguments to
    /// parameters by position and initialize locals to their type
    /// defaults.
    fn make_frame(
        &mut self,
        target: CallTarget<'_>,
        function: Function,
        instance: Option<InstanceId>,
        args: Vec<RuntimeValue>,
        return_dest: Option<String>,
    ) -> Result<Frame, RuntimeError> {
        if args.len() != function.params.len() {
            return Err(RuntimeError::WrongArgCount {
                script: target.script.to_string(),
                function: target.name.to_string(),
                expected: function.params.len(),
                got: args.len(),
            });
        }
        let mut locals = HashMap::new();
        for (param, value) in function.params.iter().zip(args) {
            locals.insert(param.name.clone(), value);
        }
        for local in &function.locals {
            locals.insert(
                local.name.clone(),
                RuntimeValue::default_for_type(&local.type_name, &mut self.interner),
            );
        }
        Ok(Frame {
            function,
            script: target.script.to_string(),
            state: target.state.to_string(),
            name: target.name.to_string(),
            instance,
            pc: 0,
            locals,
            return_dest,
        })
    }

    /// Resolve an instruction argument to a runtime value:
    /// identifiers read the named variable (locals, then instance
    /// variables, then `Self`); anything else is a literal.
    fn resolve_arg(&mut self, stack: &Stack, arg: &Value) -> Result<RuntimeValue, RuntimeError> {
        Ok(match arg {
            Value::None => RuntimeValue::None,
            Value::Identifier(name) => self.read_var(stack, name)?,
            Value::Str(text) => RuntimeValue::Str(self.interner.intern(text)),
            Value::Int(n) => RuntimeValue::Int(*n),
            Value::Float(f) => RuntimeValue::Float(*f),
            Value::Bool(b) => RuntimeValue::Bool(*b),
        })
    }

    /// Read a variable: locals first, then the instance's script
    /// variables. `Self` reads the instance itself.
    fn read_var(&self, stack: &Stack, name: &str) -> Result<RuntimeValue, RuntimeError> {
        let frame = stack.frames.last().expect("read with no frame");
        if name.eq_ignore_ascii_case("Self") {
            return match frame.instance {
                Some(id) => Ok(RuntimeValue::Object(id)),
                None => Err(RuntimeError::UndefinedVariable {
                    name: "Self (in a global function)".to_string(),
                }),
            };
        }
        if let Some(value) = frame
            .locals
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
        {
            return Ok(value);
        }
        if let Some(id) = frame.instance {
            if let Some(instance) = self.instances.get(id) {
                if let Some(value) = instance.get(name) {
                    return Ok(value.clone());
                }
            }
        }
        Err(RuntimeError::UndefinedVariable {
            name: name.to_string(),
        })
    }

    /// Write a variable: a parameter or local of the current
    /// function goes to the frame's locals; anything else goes to
    /// the instance's script variables (created if missing — the
    /// compiler always declares, but a lenient write beats a crash
    /// on an odd script).
    fn write_var(
        &mut self,
        stack: &mut Stack,
        name: &str,
        value: RuntimeValue,
    ) -> Result<(), RuntimeError> {
        let frame = stack.frames.last_mut().expect("write with no frame");
        let is_local = frame
            .function
            .params
            .iter()
            .chain(frame.function.locals.iter())
            .any(|v| v.name.eq_ignore_ascii_case(name));
        if is_local {
            let key = frame
                .function
                .params
                .iter()
                .chain(frame.function.locals.iter())
                .find(|v| v.name.eq_ignore_ascii_case(name))
                .map(|v| v.name.clone())
                .unwrap_or_else(|| name.to_string());
            frame.locals.insert(key, value);
            return Ok(());
        }
        match frame.instance {
            Some(id) => {
                if let Some(instance) = self.instances.get_mut(id) {
                    // `::state` is the compiler's implicit state
                    // variable: writing it changes the instance's
                    // state (this is how the compiler-generated
                    // `GotoState`, present in every real script,
                    // works). Keep the instance state in sync so
                    // state-aware function resolution follows the
                    // script.
                    if name.eq_ignore_ascii_case("::state") {
                        if let RuntimeValue::Str(s) = &value {
                            instance.state = s.to_string();
                        }
                    }
                    instance.set(name, value);
                    Ok(())
                } else {
                    Err(RuntimeError::UnknownInstance { id })
                }
            }
            None => Err(RuntimeError::UndefinedVariable {
                name: name.to_string(),
            }),
        }
    }

    /// The declared type of a variable, for the `cast` opcode and
    /// `array_create`: parameters and locals of the current
    /// function first, then the instance's script variables and
    /// properties (walking the parent chain).
    fn declared_type(&self, stack: &Stack, name: &str) -> Option<String> {
        let frame = stack.frames.last()?;
        if let Some(var) = frame
            .function
            .params
            .iter()
            .chain(frame.function.locals.iter())
            .find(|v| v.name.eq_ignore_ascii_case(name))
        {
            return Some(var.type_name.clone());
        }
        let id = frame.instance?;
        let script = self.instances.get(id)?.script.clone();
        if let Some((_, variable)) = resolve_variable(&self.scripts, &script, name) {
            return Some(variable.type_name.clone());
        }
        if let Some((_, property)) = resolve_property(&self.scripts, &script, name) {
            return Some(property.type_name.clone());
        }
        None
    }

    /// Invoke a native by (script, function), checking its arity
    /// first. Returns the native's outcome.
    fn call_native(
        &mut self,
        script: &str,
        name: &str,
        caller: Option<InstanceId>,
        args: Vec<RuntimeValue>,
    ) -> Result<NativeOutcome, RuntimeError> {
        let def = self
            .natives
            .get(script, name)
            .ok_or_else(|| RuntimeError::MissingNative {
                script: script.to_string(),
                function: name.to_string(),
            })?;
        if args.len() != def.arity {
            return Err(RuntimeError::WrongArgCount {
                script: script.to_string(),
                function: name.to_string(),
                expected: def.arity,
                got: args.len(),
            });
        }
        let func = def.func;
        func(self, caller, args)
    }

    /// Call a method on an instance: script functions first
    /// (state-aware), then natives. Script calls push a frame;
    /// native calls run immediately (a latent native parks the
    /// stack instead).
    fn invoke(
        &mut self,
        stack: &mut Stack,
        target: CallTarget<'_>,
        instance: InstanceId,
        args: Vec<RuntimeValue>,
        dest: Option<String>,
    ) -> Result<Step, RuntimeError> {
        let resolved = resolve_function(&self.scripts, target.script, target.state, target.name)
            .map(|r| (r.owner.name.clone(), r.function.clone()));
        if let Some((owner, function)) = resolved {
            if function.is_native() {
                return self.invoke_native(
                    stack,
                    NativeTarget {
                        script: &owner,
                        name: target.name,
                    },
                    Some(instance),
                    args,
                    dest,
                    true,
                );
            }
            let frame = self.make_frame(
                CallTarget {
                    script: &owner,
                    state: target.state,
                    name: target.name,
                },
                function,
                Some(instance),
                args,
                dest,
            )?;
            stack.frames.push(frame);
            return Ok(Step::Continue);
        }
        // State-transition events are optional: a script that does
        // not define `OnBeginState` / `OnEndState` in the current
        // state simply doesn't run them. The compiler-generated
        // `GotoState` (present in every real script) calls both
        // unconditionally, so a missing callee here is a no-op,
        // not an error — the same rule `post_event` applies to
        // events. (Inferred in M7.1 from the generated code, not
        // observed at runtime in the real game.)
        if target.name.eq_ignore_ascii_case("OnBeginState")
            || target.name.eq_ignore_ascii_case("OnEndState")
        {
            if let Some(dest) = dest {
                self.write_var(stack, &dest, RuntimeValue::None)?;
            }
            return Ok(Step::Continue);
        }
        self.invoke_native(
            stack,
            NativeTarget {
                script: target.script,
                name: target.name,
            },
            Some(instance),
            args,
            dest,
            true,
        )
    }

    /// Invoke a native, optionally falling back to the `Script`
    /// base methods (so `GotoState` and friends work on any
    /// object). The fallback applies to method calls only — a
    /// static call names its script explicitly, so falling back
    /// there would call a base method on the wrong `Self`.
    fn invoke_native(
        &mut self,
        stack: &mut Stack,
        target: NativeTarget<'_>,
        instance: Option<InstanceId>,
        args: Vec<RuntimeValue>,
        dest: Option<String>,
        allow_script_fallback: bool,
    ) -> Result<Step, RuntimeError> {
        let outcome = self.call_native_or_script_base(
            target.script,
            target.name,
            instance,
            args,
            allow_script_fallback,
        )?;
        match outcome {
            NativeOutcome::Value(value) => {
                if let Some(dest) = dest {
                    self.write_var(stack, &dest, value)?;
                }
                Ok(Step::Continue)
            }
            NativeOutcome::Park { secs, resume } => Ok(Step::Park { secs, resume, dest }),
        }
    }

    /// Call a native by (script, function), falling back to the
    /// `Script` base when asked. When both lookups miss, the error
    /// names the original script, not the fallback.
    fn call_native_or_script_base(
        &mut self,
        script: &str,
        name: &str,
        target: Option<InstanceId>,
        args: Vec<RuntimeValue>,
        allow_script_fallback: bool,
    ) -> Result<NativeOutcome, RuntimeError> {
        match self.call_native(script, name, target, args.clone()) {
            Ok(outcome) => Ok(outcome),
            Err(RuntimeError::MissingNative { .. })
                if allow_script_fallback && !script.eq_ignore_ascii_case("Script") =>
            {
                match self.call_native("Script", name, target, args) {
                    Ok(outcome) => Ok(outcome),
                    Err(RuntimeError::MissingNative { .. }) => Err(RuntimeError::MissingNative {
                        script: script.to_string(),
                        function: name.to_string(),
                    }),
                    Err(e) => Err(e),
                }
            }
            Err(e) => Err(e),
        }
    }

    /// Execute one fetched instruction.
    fn execute(
        &mut self,
        stack: &mut Stack,
        instruction: &dovah_pex::Instruction,
    ) -> Result<Step, RuntimeError> {
        let args = &instruction.args;
        match instruction.op {
            0x00 => Ok(Step::Continue), // nop
            0x01 | 0x03 | 0x05 | 0x07 | 0x09 => self.binary_int(stack, instruction.op, args),
            0x02 | 0x04 | 0x06 | 0x08 => self.binary_float(stack, instruction.op, args),
            0x0A => {
                // not: dest = !truthy(value). The operand is
                // NOT always a bool — real scripts `not`
                // objects (`if (!myRef)`) and ints (`if
                // (!myStage)`), testing for None and 0. See
                // RuntimeValue::truthy.
                let dest = dest_name(args)?;
                let value = self.resolve_arg(stack, &args[1])?;
                self.write_var(stack, &dest, RuntimeValue::Bool(!value.truthy()))?;
                Ok(Step::Continue)
            }
            0x0B => {
                let dest = dest_name(args)?;
                let value = self.resolve_arg(stack, &args[1])?;
                let RuntimeValue::Int(n) = value else {
                    return Err(RuntimeError::TypeMismatch {
                        expected: "Int".to_string(),
                        found: value.type_name().to_string(),
                    });
                };
                self.write_var(stack, &dest, RuntimeValue::Int(-n))?;
                Ok(Step::Continue)
            }
            0x0C => {
                let dest = dest_name(args)?;
                let value = self.resolve_arg(stack, &args[1])?;
                let RuntimeValue::Float(f) = value else {
                    return Err(RuntimeError::TypeMismatch {
                        expected: "Float".to_string(),
                        found: value.type_name().to_string(),
                    });
                };
                self.write_var(stack, &dest, RuntimeValue::Float(-f))?;
                Ok(Step::Continue)
            }
            0x0D => {
                // assign: dest = value
                let dest = dest_name(args)?;
                let value = self.resolve_arg(stack, &args[1])?;
                self.write_var(stack, &dest, value)?;
                Ok(Step::Continue)
            }
            0x0E => {
                // cast: dest = value as (declared type of dest)
                let dest = dest_name(args)?;
                let value = self.resolve_arg(stack, &args[1])?;
                let type_name = self
                    .declared_type(stack, &dest)
                    .ok_or_else(|| RuntimeError::CastWithoutType { dest: dest.clone() })?;
                let cast = value
                    .cast_to(&type_name, self)
                    .map_err(|_| RuntimeError::BadCast {
                        from: value.type_name().to_string(),
                        to: type_name.clone(),
                    })?;
                self.write_var(stack, &dest, cast)?;
                Ok(Step::Continue)
            }
            0x0F..=0x13 => self.compare(stack, instruction.op, args),
            0x14 => {
                // jmp
                let label = self.jump_label(stack, &args[0])?;
                let pc = stack.frames.last().expect("jmp with no frame").pc - 1;
                let count = stack
                    .frames
                    .last()
                    .expect("jmp with no frame")
                    .function
                    .instructions
                    .len();
                let target = self.semantics.jump_target(pc, label, count)?;
                stack.frames.last_mut().expect("jmp with no frame").pc = target;
                Ok(Step::Continue)
            }
            0x15 | 0x16 => {
                // jmpt / jmpf
                let condition = self.resolve_arg(stack, &args[0])?;
                let take = if instruction.op == 0x15 {
                    condition.truthy()
                } else {
                    !condition.truthy()
                };
                if take {
                    let label = self.jump_label(stack, &args[1])?;
                    let pc = stack.frames.last().expect("jmp with no frame").pc - 1;
                    let count = stack
                        .frames
                        .last()
                        .expect("jmp with no frame")
                        .function
                        .instructions
                        .len();
                    let target = self.semantics.jump_target(pc, label, count)?;
                    stack.frames.last_mut().expect("jmp with no frame").pc = target;
                }
                Ok(Step::Continue)
            }
            0x17 => self.callmethod(stack, args),
            0x18 => self.callparent(stack, args),
            0x19 => self.callstatic(stack, args),
            0x1A => {
                // return
                let value = self.resolve_arg(stack, &args[0])?;
                self.return_from_frame(stack, value)
            }
            0x1B => {
                // strcat: dest = lhs + rhs (both formatted as strings)
                let dest = dest_name(args)?;
                let lhs = self.resolve_arg(stack, &args[1])?;
                let rhs = self.resolve_arg(stack, &args[2])?;
                let joined = format!(
                    "{}{}",
                    self.format_for_concat(&lhs),
                    self.format_for_concat(&rhs)
                );
                let interned = self.interner.intern(&joined);
                self.write_var(stack, &dest, RuntimeValue::Str(interned))?;
                Ok(Step::Continue)
            }
            0x1C => self.propget(stack, args),
            0x1D => self.propset(stack, args),
            0x1E => {
                // array_create: dest = new array of `size` defaults
                let dest = dest_name(args)?;
                let size = self.resolve_arg(stack, &args[1])?;
                let RuntimeValue::Int(size) = size else {
                    return Err(RuntimeError::TypeMismatch {
                        expected: "Int".to_string(),
                        found: size.type_name().to_string(),
                    });
                };
                let type_name = self
                    .declared_type(stack, &dest)
                    .ok_or_else(|| RuntimeError::CastWithoutType { dest: dest.clone() })?;
                let Some(element) = type_name.strip_suffix("[]") else {
                    return Err(RuntimeError::NotAnArrayType {
                        dest: dest.clone(),
                        type_name,
                    });
                };
                let element = element.to_string();
                let mut items = Vec::new();
                for _ in 0..size.max(0) {
                    items.push(RuntimeValue::default_for_type(&element, &mut self.interner));
                }
                self.write_var(
                    stack,
                    &dest,
                    RuntimeValue::Array(Rc::new(RefCell::new(items))),
                )?;
                Ok(Step::Continue)
            }
            0x1F => {
                // array_length: dest = array length
                let dest = dest_name(args)?;
                let array = self.resolve_arg(stack, &args[1])?;
                let RuntimeValue::Array(items) = array else {
                    return Err(RuntimeError::NotAnArray);
                };
                let len = items.borrow().len() as i32;
                self.write_var(stack, &dest, RuntimeValue::Int(len))?;
                Ok(Step::Continue)
            }
            0x20 => {
                // array_getelement: dest = array[index] (None when out of bounds)
                let dest = dest_name(args)?;
                let array = self.resolve_arg(stack, &args[1])?;
                let index = self.resolve_arg(stack, &args[2])?;
                let RuntimeValue::Array(items) = array else {
                    return Err(RuntimeError::NotAnArray);
                };
                let RuntimeValue::Int(index) = index else {
                    return Err(RuntimeError::TypeMismatch {
                        expected: "Int".to_string(),
                        found: index.type_name().to_string(),
                    });
                };
                let value = if index < 0 {
                    RuntimeValue::None
                } else {
                    items
                        .borrow()
                        .get(index as usize)
                        .cloned()
                        .unwrap_or(RuntimeValue::None)
                };
                self.write_var(stack, &dest, value)?;
                Ok(Step::Continue)
            }
            0x21 => {
                // array_setelement: array[index] = value (no-op when out of bounds)
                let array = self.resolve_arg(stack, &args[0])?;
                let index = self.resolve_arg(stack, &args[1])?;
                let value = self.resolve_arg(stack, &args[2])?;
                let RuntimeValue::Array(items) = array else {
                    return Err(RuntimeError::NotAnArray);
                };
                let RuntimeValue::Int(index) = index else {
                    return Err(RuntimeError::TypeMismatch {
                        expected: "Int".to_string(),
                        found: index.type_name().to_string(),
                    });
                };
                if index >= 0 {
                    if let Some(slot) = items.borrow_mut().get_mut(index as usize) {
                        *slot = value;
                    }
                }
                Ok(Step::Continue)
            }
            0x22 | 0x23 => self.array_find(stack, instruction.op, args),
            _ => Err(RuntimeError::BadOperands {
                op: "unknown",
                detail: "the parser rejects unknown opcodes, so this cannot happen",
            }),
        }
    }

    /// Format a value for `strcat`: objects format as their script
    /// name, everything else by the standard string rules.
    fn format_for_concat(&self, value: &RuntimeValue) -> String {
        match value {
            RuntimeValue::Object(id) => self
                .instances
                .get(*id)
                .map(|i| i.script.clone())
                .unwrap_or_else(|| "Object".to_string()),
            _ => value.to_papyrus_string(),
        }
    }

    /// Read a jump label: an integer literal, or an identifier
    /// resolving to an integer.
    fn jump_label(&mut self, stack: &Stack, arg: &Value) -> Result<i32, RuntimeError> {
        match self.resolve_arg(stack, arg)? {
            RuntimeValue::Int(n) => Ok(n),
            other => Err(RuntimeError::TypeMismatch {
                expected: "Int".to_string(),
                found: other.type_name().to_string(),
            }),
        }
    }

    /// Integer arithmetic: iadd/isub/imul/idiv/imod.
    fn binary_int(
        &mut self,
        stack: &mut Stack,
        op: u8,
        args: &[Value],
    ) -> Result<Step, RuntimeError> {
        let dest = dest_name(args)?;
        let lhs = self.resolve_arg(stack, &args[1])?;
        let rhs = self.resolve_arg(stack, &args[2])?;
        let (RuntimeValue::Int(a), RuntimeValue::Int(b)) = (&lhs, &rhs) else {
            return Err(RuntimeError::TypeMismatch {
                expected: "Int, Int".to_string(),
                found: format!("{}, {}", lhs.type_name(), rhs.type_name()),
            });
        };
        let result = match op {
            0x01 => a.wrapping_add(*b),
            0x03 => a.wrapping_sub(*b),
            0x05 => a.wrapping_mul(*b),
            0x07 => {
                if *b == 0 {
                    return Err(RuntimeError::DivisionByZero);
                }
                a / b
            }
            0x09 => {
                if *b == 0 {
                    return Err(RuntimeError::DivisionByZero);
                }
                a % b
            }
            _ => unreachable!("binary_int called with a non-int opcode"),
        };
        self.write_var(stack, &dest, RuntimeValue::Int(result))?;
        Ok(Step::Continue)
    }

    /// Float arithmetic: fadd/fsub/fmul/fdiv.
    fn binary_float(
        &mut self,
        stack: &mut Stack,
        op: u8,
        args: &[Value],
    ) -> Result<Step, RuntimeError> {
        let dest = dest_name(args)?;
        let lhs = self.resolve_arg(stack, &args[1])?;
        let rhs = self.resolve_arg(stack, &args[2])?;
        let (RuntimeValue::Float(a), RuntimeValue::Float(b)) = (&lhs, &rhs) else {
            return Err(RuntimeError::TypeMismatch {
                expected: "Float, Float".to_string(),
                found: format!("{}, {}", lhs.type_name(), rhs.type_name()),
            });
        };
        let result = match op {
            0x02 => a + b,
            0x04 => a - b,
            0x06 => a * b,
            0x08 => a / b,
            _ => unreachable!("binary_float called with a non-float opcode"),
        };
        self.write_var(stack, &dest, RuntimeValue::Float(result))?;
        Ok(Step::Continue)
    }

    /// Comparisons: cmp_eq/cmp_lt/cmp_le/cmp_gt/cmp_ge.
    fn compare(&mut self, stack: &mut Stack, op: u8, args: &[Value]) -> Result<Step, RuntimeError> {
        let dest = dest_name(args)?;
        let lhs = self.resolve_arg(stack, &args[1])?;
        let rhs = self.resolve_arg(stack, &args[2])?;
        let result = if op == 0x0F {
            lhs.papyrus_eq(&rhs)
        } else {
            let ordering = lhs
                .papyrus_cmp(&rhs)
                .ok_or_else(|| RuntimeError::TypeMismatch {
                    expected: "two numbers or two strings".to_string(),
                    found: format!("{}, {}", lhs.type_name(), rhs.type_name()),
                })?;
            use std::cmp::Ordering;
            match op {
                0x10 => ordering == Ordering::Less,
                0x11 => ordering != Ordering::Greater,
                0x12 => ordering == Ordering::Greater,
                0x13 => ordering != Ordering::Less,
                _ => unreachable!("compare called with a non-compare opcode"),
            }
        };
        self.write_var(stack, &dest, RuntimeValue::Bool(result))?;
        Ok(Step::Continue)
    }

    /// `callmethod`: call a method on an object. Layout (proven in
    /// M7.1, see `semantics`): `(method, object, dest, args...)`.
    fn callmethod(&mut self, stack: &mut Stack, args: &[Value]) -> Result<Step, RuntimeError> {
        let call = self.semantics.callmethod_operands(args)?;
        let object = self.resolve_arg(stack, call.object)?;
        let mut resolved_args = Vec::with_capacity(call.args.len());
        for arg in call.args {
            resolved_args.push(self.resolve_arg(stack, arg)?);
        }
        let target = match object {
            RuntimeValue::Object(id) => id,
            RuntimeValue::None => {
                return Err(RuntimeError::CallOnNone {
                    what: format!("method '{}'", call.method),
                });
            }
            other => {
                return Err(RuntimeError::TypeMismatch {
                    expected: "Object".to_string(),
                    found: other.type_name().to_string(),
                });
            }
        };
        let instance = self
            .instances
            .get(target)
            .ok_or(RuntimeError::UnknownInstance { id: target })?;
        let (script, state) = (instance.script.clone(), instance.state.clone());
        let dest = call.dest.map(str::to_string);
        self.invoke(
            stack,
            CallTarget {
                script: &script,
                state: &state,
                name: call.method,
            },
            target,
            resolved_args,
            dest,
        )
    }

    /// `callparent`: call the parent script's version of a
    /// function. Layout (proven in M7.1, see `semantics`):
    /// `(method, dest, args...)`.
    fn callparent(&mut self, stack: &mut Stack, args: &[Value]) -> Result<Step, RuntimeError> {
        let call = self.semantics.callparent_operands(args)?;
        let mut resolved_args = Vec::with_capacity(call.args.len());
        for arg in call.args {
            resolved_args.push(self.resolve_arg(stack, arg)?);
        }
        let frame = stack.frames.last().expect("callparent with no frame");
        let (script, state, target) = (frame.script.clone(), frame.state.clone(), frame.instance);
        let dest = call.dest.map(str::to_string);
        let resolved = resolve_parent_function(&self.scripts, &script, &state, call.method)
            .map(|r| (r.owner.name.clone(), r.function.clone()));
        if let Some((owner, function)) = resolved {
            if function.is_native() {
                return self.invoke_native(
                    stack,
                    NativeTarget {
                        script: &owner,
                        name: call.method,
                    },
                    target,
                    resolved_args,
                    dest,
                    true,
                );
            }
            let frame = self.make_frame(
                CallTarget {
                    script: &owner,
                    state: &state,
                    name: call.method,
                },
                function,
                target,
                resolved_args,
                dest,
            )?;
            stack.frames.push(frame);
            return Ok(Step::Continue);
        }
        // No parent version: fall back to natives (the parent
        // script name, then the Script base).
        let parent = self
            .scripts
            .object(&script)
            .map(|o| o.parent.clone())
            .unwrap_or_default();
        self.invoke_native(
            stack,
            NativeTarget {
                script: &parent,
                name: call.method,
            },
            target,
            resolved_args,
            dest,
            true,
        )
    }

    /// `callstatic`: call a global function on a named script.
    /// Layout (proven in M7.1, see `semantics`): `(script, method,
    /// dest, args...)`.
    fn callstatic(&mut self, stack: &mut Stack, args: &[Value]) -> Result<Step, RuntimeError> {
        let call = self.semantics.callstatic_operands(args)?;
        let mut resolved_args = Vec::with_capacity(call.args.len());
        for arg in call.args {
            resolved_args.push(self.resolve_arg(stack, arg)?);
        }
        let dest = call.dest.map(str::to_string);
        // Global functions live in the empty state. The caller is
        // the current frame's instance (for natives that need a
        // `Self`, e.g. `Game.GetPlayer` does not, but a static call
        // made from inside a method still has one).
        let caller = stack.frames.last().and_then(|f| f.instance);
        let resolved = resolve_function(&self.scripts, call.script, "", call.method)
            .map(|r| (r.owner.name.clone(), r.function.clone()));
        if let Some((owner, function)) = resolved {
            if function.is_native() {
                return self.invoke_native(
                    stack,
                    NativeTarget {
                        script: &owner,
                        name: call.method,
                    },
                    caller,
                    resolved_args,
                    dest,
                    false,
                );
            }
            // A global function runs with no `Self`.
            let frame = self.make_frame(
                CallTarget {
                    script: &owner,
                    state: "",
                    name: call.method,
                },
                function,
                None,
                resolved_args,
                dest,
            )?;
            stack.frames.push(frame);
            return Ok(Step::Continue);
        }
        self.invoke_native(
            stack,
            NativeTarget {
                script: call.script,
                name: call.method,
            },
            caller,
            resolved_args,
            dest,
            false,
        )
    }

    /// `propget`: read an instance property into a variable.
    /// Layout (proven in M7.1 against real scripts, e.g.
    /// `AbTGTrapsightScript.OnEffectStart`: `propget Value,
    /// ::pTSight_var, ::temp1`): `(property, object, dest)`.
    fn propget(&mut self, stack: &mut Stack, args: &[Value]) -> Result<Step, RuntimeError> {
        let property = match &args[0] {
            Value::Identifier(name) => name.clone(),
            _ => {
                return Err(RuntimeError::BadOperands {
                    op: "propget",
                    detail: "property name is not an identifier",
                });
            }
        };
        let object = self.resolve_arg(stack, &args[1])?;
        let dest = match &args[2] {
            Value::Identifier(name) => name.clone(),
            _ => {
                return Err(RuntimeError::BadOperands {
                    op: "propget",
                    detail: "destination is not an identifier",
                });
            }
        };
        let target = self.object_operand(object, &format!("property '{property}'"))?;
        let instance = self
            .instances
            .get(target)
            .ok_or(RuntimeError::UnknownInstance { id: target })?;
        let script = instance.script.clone();
        let (_, definition) =
            resolve_property(&self.scripts, &script, &property).ok_or_else(|| {
                RuntimeError::MissingProperty {
                    script: script.clone(),
                    property: property.clone(),
                }
            })?;
        if !definition.can_read() {
            return Err(RuntimeError::PropertyAccess { script, property });
        }
        if definition.is_auto() {
            let var = definition.auto_var.clone().unwrap_or_default();
            let value = self
                .instances
                .get(target)
                .and_then(|i| i.get(&var).cloned())
                .unwrap_or(RuntimeValue::None);
            self.write_var(stack, &dest, value)?;
            Ok(Step::Continue)
        } else {
            let Some(handler) = definition.read_handler.clone() else {
                return Err(RuntimeError::PropertyAccess { script, property });
            };
            let frame = self.make_frame(
                CallTarget {
                    script: &script,
                    state: "",
                    name: &format!("{property} getter"),
                },
                handler,
                Some(target),
                vec![],
                Some(dest),
            )?;
            stack.frames.push(frame);
            Ok(Step::Continue)
        }
    }

    /// `propset`: write an instance property.
    /// Layout (proven in M7.1 against real scripts, e.g.
    /// `CWScript`: `propset Value, ::CWAttacker_var, ::temp475`):
    /// `(property, object, value)`.
    fn propset(&mut self, stack: &mut Stack, args: &[Value]) -> Result<Step, RuntimeError> {
        let property = match &args[0] {
            Value::Identifier(name) => name.clone(),
            _ => {
                return Err(RuntimeError::BadOperands {
                    op: "propset",
                    detail: "property name is not an identifier",
                });
            }
        };
        let object = self.resolve_arg(stack, &args[1])?;
        let value = self.resolve_arg(stack, &args[2])?;
        let target = self.object_operand(object, &format!("property '{property}'"))?;
        let instance = self
            .instances
            .get(target)
            .ok_or(RuntimeError::UnknownInstance { id: target })?;
        let script = instance.script.clone();
        let (_, definition) =
            resolve_property(&self.scripts, &script, &property).ok_or_else(|| {
                RuntimeError::MissingProperty {
                    script: script.clone(),
                    property: property.clone(),
                }
            })?;
        if !definition.can_write() {
            return Err(RuntimeError::PropertyAccess { script, property });
        }
        if definition.is_auto() {
            let var = definition.auto_var.clone().unwrap_or_default();
            if let Some(instance) = self.instances.get_mut(target) {
                instance.set(&var, value);
            }
            Ok(Step::Continue)
        } else {
            let Some(handler) = definition.write_handler.clone() else {
                return Err(RuntimeError::PropertyAccess { script, property });
            };
            let frame = self.make_frame(
                CallTarget {
                    script: &script,
                    state: "",
                    name: &format!("{property} setter"),
                },
                handler,
                Some(target),
                vec![value],
                None,
            )?;
            stack.frames.push(frame);
            Ok(Step::Continue)
        }
    }

    /// `array_findelement` / `array_rfindelement`: find an element's
    /// index, or -1. Layout (proven in M7.1 for `findelement`, e.g.
    /// `BYOHHouseBuildingScript.SetLumbermillOperator`:
    /// `array_findelement ::LumberVendors_var, ::temp84, NPC, 0`,
    /// where `::temp84` is then used as the found index):
    /// `(array, dest, needle, start)`. No `array_rfindelement`
    /// occurs in the 13,494 real scripts, so it is assumed to
    /// share the layout. The 4th argument is the start index
    /// (default 0 for forward search, -1 meaning the last element
    /// for reverse search).
    fn array_find(
        &mut self,
        stack: &mut Stack,
        op: u8,
        args: &[Value],
    ) -> Result<Step, RuntimeError> {
        let array = self.resolve_arg(stack, &args[0])?;
        let dest = match &args[1] {
            Value::Identifier(name) => name.clone(),
            _ => {
                return Err(RuntimeError::BadOperands {
                    op: "array_find",
                    detail: "destination is not an identifier",
                });
            }
        };
        let needle = self.resolve_arg(stack, &args[2])?;
        let start = self.resolve_arg(stack, &args[3])?;
        let RuntimeValue::Array(items) = array else {
            return Err(RuntimeError::NotAnArray);
        };
        let RuntimeValue::Int(start) = start else {
            return Err(RuntimeError::TypeMismatch {
                expected: "Int".to_string(),
                found: start.type_name().to_string(),
            });
        };
        let items = items.borrow();
        let found = if op == 0x22 {
            let start = start.max(0) as usize;
            items
                .iter()
                .skip(start)
                .position(|item| item.papyrus_eq(&needle))
                .map(|pos| (pos + start) as i32)
                .unwrap_or(-1)
        } else {
            let end = if start < 0 {
                items.len()
            } else {
                (start as usize + 1).min(items.len())
            };
            items
                .iter()
                .take(end)
                .rposition(|item| item.papyrus_eq(&needle))
                .map(|pos| pos as i32)
                .unwrap_or(-1)
        };
        self.write_var(stack, &dest, RuntimeValue::Int(found))?;
        Ok(Step::Continue)
    }

    /// Unwrap an object operand: an `Object` value, or an error
    /// naming what was attempted on `None` / a non-object.
    fn object_operand(&self, value: RuntimeValue, what: &str) -> Result<InstanceId, RuntimeError> {
        match value {
            RuntimeValue::Object(id) => Ok(id),
            RuntimeValue::None => Err(RuntimeError::CallOnNone {
                what: what.to_string(),
            }),
            other => Err(RuntimeError::TypeMismatch {
                expected: "Object".to_string(),
                found: other.type_name().to_string(),
            }),
        }
    }
}

/// What one interpreter step did.
#[derive(Debug)]
enum Step {
    /// Keep executing the stack.
    Continue,
    /// A latent native parked the stack.
    Park {
        secs: f64,
        resume: RuntimeValue,
        dest: Option<String>,
    },
}

impl CastContext for Vm {
    fn is_subtype_of(&self, id: InstanceId, target: &str) -> bool {
        self.instances
            .get(id)
            .map(|i| self.scripts.is_subtype(&i.script, target))
            .unwrap_or(false)
    }

    fn object_type_name(&self, id: InstanceId) -> Option<String> {
        self.instances.get(id).map(|i| i.script.clone())
    }
}

impl Default for Vm {
    fn default() -> Self {
        Vm::new()
    }
}

/// The destination of a three-address instruction: the first
/// argument, which must be an identifier.
fn dest_name(args: &[Value]) -> Result<String, RuntimeError> {
    match &args[0] {
        Value::Identifier(name) => Ok(name.clone()),
        _ => Err(RuntimeError::BadOperands {
            op: "instruction",
            detail: "destination is not an identifier",
        }),
    }
}
