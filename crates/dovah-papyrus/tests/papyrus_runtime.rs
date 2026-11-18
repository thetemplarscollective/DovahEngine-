//! End-to-end tests for the Papyrus runtime: synthetic scripts
//! built by hand (parsed `PexObject`s, no `.pex` bytes — the parser
//! is tested in dovah-pex), executed by the VM, with results read
//! back from instance variables and the trace log.
//!
//! The hand-built bytecode follows the layouts proven in M7.1
//! against the real scripts (see `dovah_papyrus::semantics`):
//! jumps are relative to the jump instruction, calls are
//! method-first with the destination last, `propget` is
//! (property, object, dest), `propset` is (property, object,
//! value), and `array_findelement` is (array, dest, needle,
//! start).

use dovah_esm::vmad::{Vmad, VmadProperty, VmadScript, VmadValue};
use dovah_papyrus::error::RuntimeError;
use dovah_papyrus::value::RuntimeValue;
use dovah_papyrus::Vm;
use dovah_pex::{
    Function, Instruction, NamedFunction, PexObject, Property, State, Value, VarType, Variable,
    FUNCTION_FLAG_GLOBAL, PROPERTY_FLAG_AUTOVAR, PROPERTY_FLAG_READ, PROPERTY_FLAG_WRITE,
};
use std::rc::Rc;

// ---- builders ------------------------------------------------------

fn id(name: &str) -> Value {
    Value::Identifier(name.to_string())
}

fn s(text: &str) -> Value {
    Value::Str(text.to_string())
}

fn i(n: i32) -> Value {
    Value::Int(n)
}

fn f(n: f32) -> Value {
    Value::Float(n)
}

fn b(v: bool) -> Value {
    Value::Bool(v)
}

fn instr(op: u8, args: Vec<Value>) -> Instruction {
    Instruction { op, args }
}

fn func(
    return_type: &str,
    params: &[(&str, &str)],
    locals: &[(&str, &str)],
    instructions: Vec<Instruction>,
) -> Function {
    func_flagged(return_type, params, locals, instructions, 0)
}

fn func_flagged(
    return_type: &str,
    params: &[(&str, &str)],
    locals: &[(&str, &str)],
    instructions: Vec<Instruction>,
    flags: u8,
) -> Function {
    Function {
        return_type: return_type.to_string(),
        docstring: String::new(),
        user_flags: 0,
        flags,
        params: params
            .iter()
            .map(|(n, t)| VarType {
                name: n.to_string(),
                type_name: t.to_string(),
            })
            .collect(),
        locals: locals
            .iter()
            .map(|(n, t)| VarType {
                name: n.to_string(),
                type_name: t.to_string(),
            })
            .collect(),
        instructions,
    }
}

fn variable(name: &str, type_name: &str, default: Value) -> Variable {
    Variable {
        name: name.to_string(),
        type_name: type_name.to_string(),
        user_flags: 0,
        default,
    }
}

fn auto_property(name: &str, type_name: &str, auto_var: &str) -> Property {
    Property {
        name: name.to_string(),
        type_name: type_name.to_string(),
        docstring: String::new(),
        user_flags: 0,
        flags: PROPERTY_FLAG_READ | PROPERTY_FLAG_WRITE | PROPERTY_FLAG_AUTOVAR,
        auto_var: Some(auto_var.to_string()),
        read_handler: None,
        write_handler: None,
    }
}

fn full_property(
    name: &str,
    type_name: &str,
    read_handler: Function,
    write_handler: Function,
) -> Property {
    Property {
        name: name.to_string(),
        type_name: type_name.to_string(),
        docstring: String::new(),
        user_flags: 0,
        flags: PROPERTY_FLAG_READ | PROPERTY_FLAG_WRITE,
        auto_var: None,
        read_handler: Some(read_handler),
        write_handler: Some(write_handler),
    }
}

fn state(name: &str, functions: Vec<(&str, Function)>) -> State {
    State {
        name: name.to_string(),
        functions: functions
            .into_iter()
            .map(|(name, function)| NamedFunction {
                name: name.to_string(),
                function,
            })
            .collect(),
    }
}

fn script(
    name: &str,
    parent: &str,
    variables: Vec<Variable>,
    properties: Vec<Property>,
    states: Vec<State>,
) -> PexObject {
    PexObject {
        name: name.to_string(),
        parent: parent.to_string(),
        docstring: String::new(),
        user_flags: 0,
        auto_state: String::new(),
        variables,
        properties,
        states,
    }
}

/// `Debug.Trace("text", 0)` as a callstatic instruction (return
/// discarded). `Trace` takes two arguments (text, severity).
fn trace(text: &str) -> Instruction {
    instr(
        0x19,
        vec![id("Debug"), id("Trace"), Value::None, s(text), i(0)],
    )
}

fn ret_none() -> Instruction {
    instr(0x1A, vec![Value::None])
}

fn vm_with(scripts: Vec<PexObject>) -> Vm {
    let mut vm = Vm::new();
    for object in scripts {
        vm.load_object(object);
    }
    vm
}

fn var(vm: &Vm, id: dovah_papyrus::InstanceId, name: &str) -> RuntimeValue {
    vm.instances
        .get(id)
        .and_then(|i| i.get(name).cloned())
        .unwrap_or(RuntimeValue::None)
}

// ---- interpreter core ----------------------------------------------

#[test]
fn add_function_returns_sum() {
    let calc = script(
        "Calc",
        "Script",
        vec![variable("result", "Int", i(0))],
        vec![],
        vec![state(
            "",
            vec![
                (
                    "Add",
                    func(
                        "Int",
                        &[("a", "Int"), ("b", "Int")],
                        &[("sum", "Int")],
                        vec![
                            instr(0x01, vec![id("sum"), id("a"), id("b")]),
                            instr(0x1A, vec![id("sum")]),
                        ],
                    ),
                ),
                (
                    "OnInit",
                    func(
                        "None",
                        &[],
                        &[("tmp", "Int")],
                        vec![
                            instr(0x17, vec![id("Add"), id("Self"), id("tmp"), i(2), i(3)]),
                            instr(0x0D, vec![id("result"), id("tmp")]),
                            ret_none(),
                        ],
                    ),
                ),
            ],
        )],
    );
    let mut vm = vm_with(vec![calc]);
    let id = vm.create_instance(0x100, "Calc", None).unwrap();
    assert!(vm.run_until_idle(100, 10));
    assert_eq!(var(&vm, id, "result"), RuntimeValue::Int(5));
    assert!(vm.errors.is_empty());
}

#[test]
fn loop_counts_to_ten() {
    // Relative jumps (proven in M7.1): the jmpt at index 3 jumps
    // back 2, to instruction 1.
    let counter = script(
        "Counter",
        "Script",
        vec![variable("count", "Int", i(0))],
        vec![],
        vec![state(
            "",
            vec![(
                "OnInit",
                func(
                    "None",
                    &[],
                    &[("done", "Bool")],
                    vec![
                        instr(0x0D, vec![id("count"), i(0)]),
                        instr(0x01, vec![id("count"), id("count"), i(1)]),
                        instr(0x10, vec![id("done"), id("count"), i(10)]),
                        instr(0x15, vec![id("done"), i(-2)]),
                        ret_none(),
                    ],
                ),
            )],
        )],
    );
    let mut vm = vm_with(vec![counter]);
    let id = vm.create_instance(0x100, "Counter", None).unwrap();
    assert!(vm.run_until_idle(100, 10));
    assert_eq!(var(&vm, id, "count"), RuntimeValue::Int(10));
    assert!(vm.errors.is_empty());
}

#[test]
fn jump_past_the_end_returns() {
    let jumper = script(
        "Jumper",
        "Script",
        vec![],
        vec![],
        vec![state(
            "",
            vec![(
                "OnInit",
                func(
                    "None",
                    &[],
                    &[],
                    vec![
                        instr(0x14, vec![i(3)]),
                        trace("skipped"),
                        trace("also skipped"),
                    ],
                ),
            )],
        )],
    );
    let mut vm = vm_with(vec![jumper]);
    vm.create_instance(0x100, "Jumper", None).unwrap();
    assert!(vm.run_until_idle(100, 10));
    assert!(vm.trace_log.is_empty());
    assert!(vm.errors.is_empty());
}

#[test]
fn cast_opcode_converts() {
    let caster = script(
        "Caster",
        "Script",
        vec![
            variable("seen_f", "Float", f(0.0)),
            variable("seen_s", "String", s("")),
        ],
        vec![],
        vec![state(
            "",
            vec![(
                "OnInit",
                func(
                    "None",
                    &[],
                    &[("n", "Int"), ("x", "Float"), ("t", "String")],
                    vec![
                        instr(0x0D, vec![id("n"), i(7)]),
                        instr(0x0E, vec![id("x"), id("n")]),
                        instr(0x0E, vec![id("t"), id("x")]),
                        instr(0x0D, vec![id("seen_f"), id("x")]),
                        instr(0x0D, vec![id("seen_s"), id("t")]),
                        ret_none(),
                    ],
                ),
            )],
        )],
    );
    let mut vm = vm_with(vec![caster]);
    let id = vm.create_instance(0x100, "Caster", None).unwrap();
    assert!(vm.run_until_idle(100, 10));
    assert_eq!(var(&vm, id, "seen_f"), RuntimeValue::Float(7.0));
    assert_eq!(var(&vm, id, "seen_s"), RuntimeValue::Str(Rc::from("7")));
    assert!(vm.errors.is_empty());
}

#[test]
fn strcat_concatenates() {
    let joiner = script(
        "Joiner",
        "Script",
        vec![variable("seen", "String", s(""))],
        vec![],
        vec![state(
            "",
            vec![(
                "OnInit",
                func(
                    "None",
                    &[],
                    &[("tmp", "String")],
                    vec![
                        instr(0x1B, vec![id("tmp"), s("Hello, "), s("world")]),
                        instr(0x0D, vec![id("seen"), id("tmp")]),
                        ret_none(),
                    ],
                ),
            )],
        )],
    );
    let mut vm = vm_with(vec![joiner]);
    let id = vm.create_instance(0x100, "Joiner", None).unwrap();
    assert!(vm.run_until_idle(100, 10));
    assert_eq!(
        var(&vm, id, "seen"),
        RuntimeValue::Str(Rc::from("Hello, world"))
    );
    assert!(vm.errors.is_empty());
}

#[test]
fn arrays_create_get_set_find() {
    let worker = script(
        "ArrayWorker",
        "Script",
        vec![
            variable("seen_len", "Int", i(0)),
            variable("seen_val", "Int", i(0)),
            variable("seen_idx", "Int", i(0)),
            variable("seen_oob", "Var", Value::None),
        ],
        vec![],
        vec![state(
            "",
            vec![(
                "OnInit",
                func(
                    "None",
                    &[],
                    &[
                        ("arr", "Int[]"),
                        ("len", "Int"),
                        ("val", "Int"),
                        ("idx", "Int"),
                        ("oob", "Var"),
                    ],
                    vec![
                        instr(0x1E, vec![id("arr"), i(3)]),
                        instr(0x21, vec![id("arr"), i(0), i(10)]),
                        instr(0x21, vec![id("arr"), i(1), i(20)]),
                        instr(0x21, vec![id("arr"), i(2), i(30)]),
                        instr(0x1F, vec![id("len"), id("arr")]),
                        instr(0x20, vec![id("val"), id("arr"), i(1)]),
                        instr(0x22, vec![id("arr"), id("idx"), i(30), i(0)]),
                        instr(0x20, vec![id("oob"), id("arr"), i(99)]),
                        instr(0x0D, vec![id("seen_len"), id("len")]),
                        instr(0x0D, vec![id("seen_val"), id("val")]),
                        instr(0x0D, vec![id("seen_idx"), id("idx")]),
                        instr(0x0D, vec![id("seen_oob"), id("oob")]),
                        ret_none(),
                    ],
                ),
            )],
        )],
    );
    let mut vm = vm_with(vec![worker]);
    let id = vm.create_instance(0x100, "ArrayWorker", None).unwrap();
    assert!(vm.run_until_idle(100, 10));
    assert_eq!(var(&vm, id, "seen_len"), RuntimeValue::Int(3));
    assert_eq!(var(&vm, id, "seen_val"), RuntimeValue::Int(20));
    assert_eq!(var(&vm, id, "seen_idx"), RuntimeValue::Int(2));
    assert_eq!(var(&vm, id, "seen_oob"), RuntimeValue::None);
    assert!(vm.errors.is_empty());
}

// ---- calls and natives ---------------------------------------------

#[test]
fn native_debug_trace_and_math() {
    let user = script(
        "MathUser",
        "Script",
        vec![variable("root", "Float", f(0.0))],
        vec![],
        vec![state(
            "",
            vec![(
                "OnInit",
                func(
                    "None",
                    &[],
                    &[("tmp", "Float")],
                    vec![
                        trace("hello"),
                        instr(0x19, vec![id("Math"), id("Sqrt"), id("tmp"), f(16.0)]),
                        instr(0x0D, vec![id("root"), id("tmp")]),
                        ret_none(),
                    ],
                ),
            )],
        )],
    );
    let mut vm = vm_with(vec![user]);
    let id = vm.create_instance(0x100, "MathUser", None).unwrap();
    assert!(vm.run_until_idle(100, 10));
    assert_eq!(vm.trace_log, vec!["hello".to_string()]);
    assert_eq!(var(&vm, id, "root"), RuntimeValue::Float(4.0));
    assert!(vm.errors.is_empty());
}

#[test]
fn random_int_is_deterministic_with_a_fixed_seed() {
    let roller = script(
        "Roller",
        "Script",
        vec![variable("roll", "Int", i(0))],
        vec![],
        vec![state(
            "",
            vec![(
                "OnInit",
                func(
                    "None",
                    &[],
                    &[("tmp", "Int")],
                    vec![
                        instr(
                            0x19,
                            vec![id("Utility"), id("RandomInt"), id("tmp"), i(1), i(100)],
                        ),
                        instr(0x0D, vec![id("roll"), id("tmp")]),
                        ret_none(),
                    ],
                ),
            )],
        )],
    );
    let run = |seed: u64| {
        let mut vm = vm_with(vec![roller.clone()]);
        vm.seed_rng(seed);
        let id = vm.create_instance(0x100, "Roller", None).unwrap();
        assert!(vm.run_until_idle(100, 10));
        var(&vm, id, "roll")
    };
    let first = run(1234);
    let second = run(1234);
    assert_eq!(first, second);
    match first {
        RuntimeValue::Int(n) => assert!((1..=100).contains(&n)),
        _ => panic!("expected an int roll"),
    }
    // A different seed gives (almost certainly) a different roll.
    assert_ne!(first, run(9999));
}

#[test]
fn global_function_via_callstatic() {
    let lib = script(
        "Lib",
        "Script",
        vec![],
        vec![],
        vec![state(
            "",
            vec![(
                "Double",
                func_flagged(
                    "Int",
                    &[("x", "Int")],
                    &[("r", "Int")],
                    vec![
                        instr(0x05, vec![id("r"), id("x"), i(2)]),
                        instr(0x1A, vec![id("r")]),
                    ],
                    FUNCTION_FLAG_GLOBAL,
                ),
            )],
        )],
    );
    let user = script(
        "User",
        "Script",
        vec![variable("seen", "Int", i(0))],
        vec![],
        vec![state(
            "",
            vec![(
                "OnInit",
                func(
                    "None",
                    &[],
                    &[("tmp", "Int")],
                    vec![
                        instr(0x19, vec![id("Lib"), id("Double"), id("tmp"), i(21)]),
                        instr(0x0D, vec![id("seen"), id("tmp")]),
                        ret_none(),
                    ],
                ),
            )],
        )],
    );
    let mut vm = vm_with(vec![lib, user]);
    let id = vm.create_instance(0x100, "User", None).unwrap();
    assert!(vm.run_until_idle(100, 10));
    assert_eq!(var(&vm, id, "seen"), RuntimeValue::Int(42));
    assert!(vm.errors.is_empty());
}

#[test]
fn callparent_calls_parent_version() {
    let base = script(
        "Base",
        "Script",
        vec![],
        vec![],
        vec![state(
            "",
            vec![(
                "Open",
                func("None", &[], &[], vec![trace("base open"), ret_none()]),
            )],
        )],
    );
    let child = script(
        "Child",
        "Base",
        vec![],
        vec![],
        vec![state(
            "",
            vec![(
                "Open",
                func(
                    "None",
                    &[],
                    &[],
                    vec![
                        instr(0x18, vec![id("Open"), Value::None]),
                        trace("child open"),
                        ret_none(),
                    ],
                ),
            )],
        )],
    );
    let mut vm = vm_with(vec![base, child]);
    let id = vm.create_instance(0x100, "Child", None).unwrap();
    assert!(vm.post_event(id, "Open", vec![]));
    assert!(vm.run_until_idle(100, 10));
    assert_eq!(
        vm.trace_log,
        vec!["base open".to_string(), "child open".to_string()]
    );
    assert!(vm.errors.is_empty());
}

// ---- properties ----------------------------------------------------

#[test]
fn auto_property_get_and_set() {
    let door = script(
        "Door",
        "Script",
        vec![
            variable("IsOpen_var", "Bool", b(false)),
            variable("seen", "Bool", b(false)),
        ],
        vec![auto_property("IsOpen", "Bool", "IsOpen_var")],
        vec![state(
            "",
            vec![(
                "OnInit",
                func(
                    "None",
                    &[],
                    &[("tmp", "Bool")],
                    vec![
                        instr(0x1D, vec![id("IsOpen"), id("Self"), b(true)]),
                        instr(0x1C, vec![id("IsOpen"), id("Self"), id("tmp")]),
                        instr(0x0D, vec![id("seen"), id("tmp")]),
                        ret_none(),
                    ],
                ),
            )],
        )],
    );
    let mut vm = vm_with(vec![door]);
    let id = vm.create_instance(0x100, "Door", None).unwrap();
    assert!(vm.run_until_idle(100, 10));
    assert_eq!(var(&vm, id, "seen"), RuntimeValue::Bool(true));
    assert_eq!(var(&vm, id, "IsOpen_var"), RuntimeValue::Bool(true));
    assert!(vm.errors.is_empty());
}

#[test]
fn full_property_handlers_run() {
    let chest = script(
        "Chest",
        "Script",
        vec![
            variable("hidden", "Int", i(0)),
            variable("seen", "Int", i(0)),
        ],
        vec![full_property(
            "Gold",
            "Int",
            func("Int", &[], &[], vec![instr(0x1A, vec![id("hidden")])]),
            func(
                "None",
                &[("value", "Int")],
                &[],
                vec![instr(0x0D, vec![id("hidden"), id("value")]), ret_none()],
            ),
        )],
        vec![state(
            "",
            vec![(
                "OnInit",
                func(
                    "None",
                    &[],
                    &[("tmp", "Int")],
                    vec![
                        instr(0x1D, vec![id("Gold"), id("Self"), i(50)]),
                        instr(0x1C, vec![id("Gold"), id("Self"), id("tmp")]),
                        instr(0x0D, vec![id("seen"), id("tmp")]),
                        ret_none(),
                    ],
                ),
            )],
        )],
    );
    let mut vm = vm_with(vec![chest]);
    let id = vm.create_instance(0x100, "Chest", None).unwrap();
    assert!(vm.run_until_idle(100, 10));
    assert_eq!(var(&vm, id, "seen"), RuntimeValue::Int(50));
    assert_eq!(var(&vm, id, "hidden"), RuntimeValue::Int(50));
    assert!(vm.errors.is_empty());
}

// ---- instances and VMAD --------------------------------------------

fn vmad_with(script_name: &str, properties: Vec<VmadProperty>) -> Vmad {
    Vmad {
        version: 5,
        obj_format: 2,
        scripts: vec![VmadScript {
            name: script_name.to_string(),
            status: 0,
            properties,
        }],
        fragments: None,
    }
}

fn vmad_prop(name: &str, value: VmadValue) -> VmadProperty {
    VmadProperty {
        name: name.to_string(),
        status: 1,
        value,
    }
}

#[test]
fn vmad_init_applies_properties_and_runs_oninit() {
    let config = script(
        "Config",
        "Script",
        vec![
            variable("health_var", "Int", i(0)),
            variable("name_var", "String", s("")),
            variable("speed_var", "Float", f(0.0)),
            variable("active_var", "Bool", b(false)),
        ],
        vec![
            auto_property("Health", "Int", "health_var"),
            auto_property("Name", "String", "name_var"),
            auto_property("Speed", "Float", "speed_var"),
            auto_property("Active", "Bool", "active_var"),
        ],
        vec![state(
            "",
            vec![(
                "OnInit",
                func("None", &[], &[], vec![trace("init"), ret_none()]),
            )],
        )],
    );
    let vmad = vmad_with(
        "Config",
        vec![
            vmad_prop("Health", VmadValue::Int(100)),
            vmad_prop("Name", VmadValue::Str("Test".to_string())),
            vmad_prop("Speed", VmadValue::Float(2.5)),
            vmad_prop("Active", VmadValue::Bool(true)),
        ],
    );
    let mut vm = vm_with(vec![config]);
    let id = vm.create_instance(0x100, "Config", Some(&vmad)).unwrap();
    assert_eq!(var(&vm, id, "health_var"), RuntimeValue::Int(100));
    assert_eq!(
        var(&vm, id, "name_var"),
        RuntimeValue::Str(Rc::from("Test"))
    );
    assert_eq!(var(&vm, id, "speed_var"), RuntimeValue::Float(2.5));
    assert_eq!(var(&vm, id, "active_var"), RuntimeValue::Bool(true));
    assert!(vm.run_until_idle(100, 10));
    assert_eq!(vm.trace_log, vec!["init".to_string()]);
    assert!(vm.errors.is_empty());
    assert!(vm.warnings.is_empty());
}

#[test]
fn vmad_object_property_resolves_to_instance() {
    let actor = script("Actor", "Script", vec![], vec![], vec![state("", vec![])]);
    let holder = script(
        "Holder",
        "Script",
        vec![variable("target_var", "Actor", Value::None)],
        vec![auto_property("Target", "Actor", "target_var")],
        vec![state("", vec![])],
    );
    let vmad = vmad_with(
        "Holder",
        vec![vmad_prop(
            "Target",
            VmadValue::Object {
                form_id: 0x200,
                alias_id: None,
            },
        )],
    );
    let mut vm = vm_with(vec![actor, holder]);
    let holder_id = vm.create_instance(0x100, "Holder", Some(&vmad)).unwrap();
    // The Actor instance on form 0x200 was created on demand.
    let actor_instances = vm.instances.on_form(0x200);
    assert_eq!(actor_instances.len(), 1);
    assert_eq!(
        var(&vm, holder_id, "target_var"),
        RuntimeValue::Object(actor_instances[0])
    );
    assert!(vm.errors.is_empty());
}

#[test]
fn vmad_alias_property_is_recorded_not_crash() {
    let holder = script(
        "Holder",
        "Script",
        vec![variable("target_var", "Actor", Value::None)],
        vec![auto_property("Target", "Actor", "target_var")],
        vec![state("", vec![])],
    );
    let vmad = vmad_with(
        "Holder",
        vec![vmad_prop(
            "Target",
            VmadValue::Object {
                form_id: 0x300,
                alias_id: Some(5),
            },
        )],
    );
    let mut vm = vm_with(vec![holder]);
    let holder_id = vm.create_instance(0x100, "Holder", Some(&vmad)).unwrap();
    // Alias resolution needs the quest alias table (future work):
    // the property stays None and the alias is recorded.
    assert_eq!(var(&vm, holder_id, "target_var"), RuntimeValue::None);
    assert_eq!(vm.unresolved_aliases.len(), 1);
    assert_eq!(
        vm.unresolved_aliases[0],
        (holder_id, "Target".to_string(), 0x300, 5)
    );
}

#[test]
fn attach_from_vmad_warns_on_missing_scripts() {
    let vmad = vmad_with("NoSuchScript", vec![]);
    let mut vm = Vm::new();
    let created = vm.attach_scripts_from_vmad(0x100, &vmad);
    assert!(created.is_empty());
    assert_eq!(vm.warnings.len(), 1);
    assert!(vm.warnings[0].contains("NoSuchScript"));
}

// ---- states --------------------------------------------------------

#[test]
fn goto_state_fires_begin_and_end_events() {
    let door = script(
        "StateDoor",
        "Script",
        vec![variable("state_seen", "String", s(""))],
        vec![],
        vec![
            state(
                "",
                vec![
                    (
                        "OnInit",
                        func(
                            "None",
                            &[],
                            &[("tmp", "String")],
                            vec![
                                trace("init"),
                                instr(
                                    0x17,
                                    vec![id("GotoState"), id("Self"), Value::None, s("Open")],
                                ),
                                instr(0x17, vec![id("GetState"), id("Self"), id("tmp")]),
                                instr(0x0D, vec![id("state_seen"), id("tmp")]),
                                ret_none(),
                            ],
                        ),
                    ),
                    (
                        "OnEndState",
                        func("None", &[], &[], vec![trace("end empty"), ret_none()]),
                    ),
                ],
            ),
            state(
                "Open",
                vec![(
                    "OnBeginState",
                    func("None", &[], &[], vec![trace("begin open"), ret_none()]),
                )],
            ),
        ],
    );
    let mut vm = vm_with(vec![door]);
    let id = vm.create_instance(0x100, "StateDoor", None).unwrap();
    assert!(vm.run_until_idle(100, 10));
    assert_eq!(
        vm.trace_log,
        vec![
            "init".to_string(),
            "end empty".to_string(),
            "begin open".to_string()
        ]
    );
    assert_eq!(
        var(&vm, id, "state_seen"),
        RuntimeValue::Str(Rc::from("Open"))
    );
    assert_eq!(vm.instances.get(id).unwrap().state, "Open");
    assert!(vm.errors.is_empty());
}

// ---- scheduling ----------------------------------------------------

#[test]
fn scheduler_time_slices_between_stacks() {
    let tracer = script(
        "Tracer",
        "Script",
        vec![variable("tag", "String", s(""))],
        vec![],
        vec![state(
            "",
            vec![(
                "OnInit",
                func(
                    "None",
                    &[],
                    &[("a", "String"), ("b", "String"), ("c", "String")],
                    vec![
                        instr(0x1B, vec![id("a"), id("tag"), s("1")]),
                        instr(
                            0x19,
                            vec![id("Debug"), id("Trace"), Value::None, id("a"), i(0)],
                        ),
                        instr(0x1B, vec![id("b"), id("tag"), s("2")]),
                        instr(
                            0x19,
                            vec![id("Debug"), id("Trace"), Value::None, id("b"), i(0)],
                        ),
                        instr(0x1B, vec![id("c"), id("tag"), s("3")]),
                        instr(
                            0x19,
                            vec![id("Debug"), id("Trace"), Value::None, id("c"), i(0)],
                        ),
                        ret_none(),
                    ],
                ),
            )],
        )],
    );
    let mut vm = vm_with(vec![tracer]);
    let a = vm.create_instance(0x100, "Tracer", None).unwrap();
    let b = vm.create_instance(0x200, "Tracer", None).unwrap();
    vm.instances
        .get_mut(a)
        .unwrap()
        .set("tag", RuntimeValue::Str(Rc::from("A")));
    vm.instances
        .get_mut(b)
        .unwrap()
        .set("tag", RuntimeValue::Str(Rc::from("B")));
    // One instruction per tick: the stacks interleave.
    assert!(vm.run_until_idle(1, 100));
    assert_eq!(
        vm.trace_log,
        vec![
            "A1".to_string(),
            "B1".to_string(),
            "A2".to_string(),
            "B2".to_string(),
            "A3".to_string(),
            "B3".to_string(),
        ]
    );
    assert!(vm.errors.is_empty());
}

#[test]
fn wait_parks_and_resumes_on_virtual_time() {
    let waiter = script(
        "Waiter",
        "Script",
        vec![],
        vec![],
        vec![state(
            "",
            vec![(
                "OnInit",
                func(
                    "None",
                    &[],
                    &[],
                    vec![
                        trace("before"),
                        instr(0x19, vec![id("Utility"), id("Wait"), Value::None, f(2.0)]),
                        trace("after"),
                        ret_none(),
                    ],
                ),
            )],
        )],
    );
    let mut vm = vm_with(vec![waiter]);
    vm.create_instance(0x100, "Waiter", None).unwrap();
    vm.run_tick(100, 0.0);
    assert_eq!(vm.trace_log, vec!["before".to_string()]);
    assert_eq!(vm.pending_stacks(), 1);
    vm.run_tick(100, 1.0);
    assert_eq!(vm.trace_log, vec!["before".to_string()]);
    assert_eq!(vm.pending_stacks(), 1);
    vm.run_tick(100, 1.0);
    assert_eq!(
        vm.trace_log,
        vec!["before".to_string(), "after".to_string()]
    );
    assert_eq!(vm.pending_stacks(), 0);
    assert!(vm.errors.is_empty());
}

#[test]
fn register_for_update_fires_repeatedly() {
    let updater = script(
        "Updater",
        "Script",
        vec![],
        vec![],
        vec![state(
            "",
            vec![
                (
                    "OnInit",
                    func(
                        "None",
                        &[],
                        &[],
                        vec![
                            instr(
                                0x17,
                                vec![id("RegisterForUpdate"), id("Self"), Value::None, f(1.0)],
                            ),
                            ret_none(),
                        ],
                    ),
                ),
                (
                    "OnUpdate",
                    func("None", &[], &[], vec![trace("tick"), ret_none()]),
                ),
            ],
        )],
    );
    let mut vm = vm_with(vec![updater]);
    vm.create_instance(0x100, "Updater", None).unwrap();
    assert!(vm.run_until_idle(100, 10));
    assert!(vm.trace_log.is_empty());
    vm.run_tick(100, 1.0);
    assert_eq!(vm.trace_log, vec!["tick".to_string()]);
    vm.run_tick(100, 1.0);
    assert_eq!(vm.trace_log, vec!["tick".to_string(), "tick".to_string()]);
    vm.run_tick(100, 0.5);
    assert_eq!(vm.trace_log.len(), 2);
    assert!(vm.errors.is_empty());
}

// ---- errors --------------------------------------------------------

#[test]
fn call_on_none_errors_but_vm_survives() {
    let boom = script(
        "Boom",
        "Script",
        vec![],
        vec![],
        vec![state(
            "",
            vec![(
                "OnInit",
                func(
                    "None",
                    &[],
                    &[],
                    vec![
                        instr(0x17, vec![id("Foo"), Value::None, Value::None]),
                        ret_none(),
                    ],
                ),
            )],
        )],
    );
    let good = script(
        "Good",
        "Script",
        vec![],
        vec![],
        vec![state(
            "",
            vec![(
                "OnInit",
                func("None", &[], &[], vec![trace("ok"), ret_none()]),
            )],
        )],
    );
    let mut vm = vm_with(vec![boom, good]);
    vm.create_instance(0x100, "Boom", None).unwrap();
    vm.create_instance(0x200, "Good", None).unwrap();
    assert!(vm.run_until_idle(100, 10));
    assert_eq!(vm.errors.len(), 1);
    assert!(matches!(vm.errors[0], RuntimeError::CallOnNone { .. }));
    assert_eq!(vm.trace_log, vec!["ok".to_string()]);
}

#[test]
fn missing_native_names_the_function() {
    let user = script(
        "SkseUser",
        "Script",
        vec![],
        vec![],
        vec![state(
            "",
            vec![(
                "OnInit",
                func(
                    "None",
                    &[],
                    &[],
                    vec![
                        instr(0x19, vec![id("SKSE"), id("SomeFunction"), Value::None]),
                        ret_none(),
                    ],
                ),
            )],
        )],
    );
    let mut vm = vm_with(vec![user]);
    vm.create_instance(0x100, "SkseUser", None).unwrap();
    assert!(vm.run_until_idle(100, 10));
    assert_eq!(vm.errors.len(), 1);
    assert_eq!(
        vm.errors[0],
        RuntimeError::MissingNative {
            script: "SKSE".to_string(),
            function: "SomeFunction".to_string(),
        }
    );
}

#[test]
fn division_by_zero_is_an_error() {
    let divider = script(
        "Divider",
        "Script",
        vec![],
        vec![],
        vec![state(
            "",
            vec![(
                "OnInit",
                func(
                    "None",
                    &[],
                    &[("r", "Int")],
                    vec![instr(0x07, vec![id("r"), i(1), i(0)]), ret_none()],
                ),
            )],
        )],
    );
    let mut vm = vm_with(vec![divider]);
    vm.create_instance(0x100, "Divider", None).unwrap();
    assert!(vm.run_until_idle(100, 10));
    assert_eq!(vm.errors, vec![RuntimeError::DivisionByZero]);
}

// ---- M7.1: real-script findings ------------------------------------

#[test]
fn script_defined_goto_state_changes_state() {
    // Real scripts carry a compiler-generated GotoState (call
    // onEndState, assign ::state, call onBeginState) and a
    // GetState that returns ::state. The VM must follow the
    // ::state assignment, and missing OnBeginState/OnEndState
    // handlers must be no-ops, not errors.
    let door = script(
        "StateDoor2",
        "Script",
        vec![variable("state_seen", "String", s(""))],
        vec![],
        vec![
            state(
                "",
                vec![
                    (
                        "GotoState",
                        func(
                            "None",
                            &[("newState", "String")],
                            &[],
                            vec![
                                instr(0x17, vec![id("onEndState"), id("Self"), Value::None]),
                                instr(0x0D, vec![id("::state"), id("newState")]),
                                instr(0x17, vec![id("onBeginState"), id("Self"), Value::None]),
                            ],
                        ),
                    ),
                    (
                        "GetState",
                        func("String", &[], &[], vec![instr(0x1A, vec![id("::state")])]),
                    ),
                    (
                        "OnInit",
                        func(
                            "None",
                            &[],
                            &[("tmp", "String")],
                            vec![
                                instr(
                                    0x17,
                                    vec![id("GotoState"), id("Self"), Value::None, s("Open")],
                                ),
                                instr(0x17, vec![id("GetState"), id("Self"), id("tmp")]),
                                instr(0x0D, vec![id("state_seen"), id("tmp")]),
                                // A second hop to a state with no
                                // handlers at all: must not error.
                                instr(
                                    0x17,
                                    vec![id("GotoState"), id("Self"), Value::None, s("Empty2")],
                                ),
                                ret_none(),
                            ],
                        ),
                    ),
                    (
                        "OnEndState",
                        func("None", &[], &[], vec![trace("end empty"), ret_none()]),
                    ),
                ],
            ),
            state(
                "Open",
                vec![(
                    "OnBeginState",
                    func("None", &[], &[], vec![trace("begin open"), ret_none()]),
                )],
            ),
        ],
    );
    let mut vm = vm_with(vec![door]);
    let id = vm.create_instance(0x100, "StateDoor2", None).unwrap();
    assert!(vm.run_until_idle(100, 10));
    // The second hop's onEndState falls back to the empty-state
    // version (resolution order); its onBeginState is missing
    // everywhere and no-ops.
    assert_eq!(
        vm.trace_log,
        vec![
            "end empty".to_string(),
            "begin open".to_string(),
            "end empty".to_string()
        ]
    );
    assert_eq!(
        var(&vm, id, "state_seen"),
        RuntimeValue::Str(Rc::from("Open"))
    );
    assert_eq!(vm.instances.get(id).unwrap().state, "Empty2");
    assert!(vm.errors.is_empty());
}

#[test]
fn m7_1_natives_random_time_math_notify() {
    let user = script(
        "NativeUser",
        "Script",
        vec![
            variable("roll_f", "Float", f(0.0)),
            variable("roll_i", "Int", i(0)),
            variable("game_time", "Float", f(0.0)),
            variable("real_time", "Float", f(0.0)),
            variable("in_menu", "Bool", b(true)),
            variable("atan1", "Float", f(0.0)),
            variable("deg", "Float", f(0.0)),
        ],
        vec![],
        vec![state(
            "",
            vec![
                (
                    "OnInit",
                    func(
                        "None",
                        &[],
                        &[
                            ("rf", "Float"),
                            ("ri", "Int"),
                            ("rt", "Float"),
                            ("mm", "Bool"),
                            ("at", "Float"),
                            ("dg", "Float"),
                        ],
                        vec![
                            instr(
                                0x19,
                                vec![id("Utility"), id("RandomFloat"), id("rf"), f(10.0), f(20.0)],
                            ),
                            instr(
                                0x19,
                                vec![id("Utility"), id("RandomInt"), id("ri"), i(1), i(6)],
                            ),
                            instr(
                                0x19,
                                vec![id("Utility"), id("GetCurrentRealTime"), id("rt")],
                            ),
                            instr(0x19, vec![id("Utility"), id("IsInMenuMode"), id("mm")]),
                            instr(0x19, vec![id("Math"), id("Atan"), id("at"), f(1.0)]),
                            instr(
                                0x19,
                                vec![id("Math"), id("DegreesToRadians"), id("dg"), f(180.0)],
                            ),
                            instr(
                                0x19,
                                vec![
                                    id("Debug"),
                                    id("Notification"),
                                    Value::None,
                                    s("hello player"),
                                ],
                            ),
                            instr(0x0D, vec![id("roll_f"), id("rf")]),
                            instr(0x0D, vec![id("roll_i"), id("ri")]),
                            instr(0x0D, vec![id("real_time"), id("rt")]),
                            instr(0x0D, vec![id("in_menu"), id("mm")]),
                            instr(0x0D, vec![id("atan1"), id("at")]),
                            instr(0x0D, vec![id("deg"), id("dg")]),
                            ret_none(),
                        ],
                    ),
                ),
                (
                    "CheckTime",
                    func(
                        "None",
                        &[],
                        &[("gt", "Float")],
                        vec![
                            instr(
                                0x19,
                                vec![id("Utility"), id("GetCurrentGameTime"), id("gt")],
                            ),
                            instr(0x0D, vec![id("game_time"), id("gt")]),
                            ret_none(),
                        ],
                    ),
                ),
            ],
        )],
    );
    let mut vm = vm_with(vec![user]);
    let id = vm.create_instance(0x100, "NativeUser", None).unwrap();
    assert!(vm.run_until_idle(100, 10));
    assert!(vm.errors.is_empty());
    match var(&vm, id, "roll_f") {
        RuntimeValue::Float(x) => assert!((10.0..20.0).contains(&x)),
        other => panic!("expected a float roll, got {other:?}"),
    }
    match var(&vm, id, "roll_i") {
        RuntimeValue::Int(n) => assert!((1..=6).contains(&n)),
        other => panic!("expected an int roll, got {other:?}"),
    }
    match var(&vm, id, "real_time") {
        RuntimeValue::Float(x) => assert!(x >= 0.0),
        other => panic!("expected a float real time, got {other:?}"),
    }
    assert_eq!(var(&vm, id, "in_menu"), RuntimeValue::Bool(false));
    match var(&vm, id, "atan1") {
        RuntimeValue::Float(x) => assert!((x - std::f32::consts::FRAC_PI_4).abs() < 1e-6),
        other => panic!("expected atan(1), got {other:?}"),
    }
    match var(&vm, id, "deg") {
        RuntimeValue::Float(x) => assert!((x - std::f32::consts::PI).abs() < 1e-6),
        other => panic!("expected pi, got {other:?}"),
    }
    assert_eq!(vm.notifications, vec!["hello player".to_string()]);
    // One game day later, GetCurrentGameTime reads 1.0.
    vm.run_tick(100, 86_400.0);
    assert!(vm.post_event(id, "CheckTime", vec![]));
    assert!(vm.run_until_idle(100, 10));
    assert_eq!(var(&vm, id, "game_time"), RuntimeValue::Float(1.0));
    assert!(vm.errors.is_empty());
}

#[test]
fn wait_game_time_parks_like_wait() {
    let waiter = script(
        "GameWaiter",
        "Script",
        vec![],
        vec![],
        vec![state(
            "",
            vec![(
                "OnInit",
                func(
                    "None",
                    &[],
                    &[],
                    vec![
                        trace("before"),
                        instr(
                            0x19,
                            vec![id("Utility"), id("WaitGameTime"), Value::None, f(2.0)],
                        ),
                        trace("after"),
                        ret_none(),
                    ],
                ),
            )],
        )],
    );
    let mut vm = vm_with(vec![waiter]);
    vm.create_instance(0x100, "GameWaiter", None).unwrap();
    vm.run_tick(100, 0.0);
    assert_eq!(vm.trace_log, vec!["before".to_string()]);
    assert_eq!(vm.pending_stacks(), 1);
    vm.run_tick(100, 2.0);
    assert_eq!(
        vm.trace_log,
        vec!["before".to_string(), "after".to_string()]
    );
    assert_eq!(vm.pending_stacks(), 0);
    assert!(vm.errors.is_empty());
}

#[test]
fn call_function_returns_value() {
    let calc = script(
        "Calc2",
        "Script",
        vec![],
        vec![],
        vec![state(
            "",
            vec![(
                "Add",
                func(
                    "Int",
                    &[("a", "Int"), ("b", "Int")],
                    &[("sum", "Int")],
                    vec![
                        instr(0x01, vec![id("sum"), id("a"), id("b")]),
                        instr(0x1A, vec![id("sum")]),
                    ],
                ),
            )],
        )],
    );
    let mut vm = vm_with(vec![calc]);
    let id = vm.create_instance(0x100, "Calc2", None).unwrap();
    assert!(vm.run_until_idle(100, 10));
    assert!(!vm.call_function(id, "Nope", vec![]));
    assert!(vm.call_function(id, "Add", vec![RuntimeValue::Int(2), RuntimeValue::Int(3)]));
    assert!(vm.run_until_idle(100, 10));
    assert_eq!(vm.last_stack_result, Some(RuntimeValue::Int(5)));
    assert!(vm.errors.is_empty());
    // A native through the same path (Script base fallback) runs
    // immediately.
    assert!(vm.call_function(id, "GetState", vec![]));
    assert_eq!(vm.last_stack_result, Some(RuntimeValue::Str(Rc::from(""))));
}
