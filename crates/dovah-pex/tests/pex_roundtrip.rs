//! Round-trip tests for the PEX parser, using synthetic scripts
//! built in memory. No real game data is involved: the builders
//! below write valid PEX bytes from the UESP spec (big-endian), and
//! the tests check the parser returns the right header, strings,
//! variables, properties, states, functions, and instructions.

use dovah_pex::{
    opcode_name, Function, Instruction, PexError, PexFile, Value, FUNCTION_FLAG_GLOBAL,
    FUNCTION_FLAG_NATIVE, MAGIC, PROPERTY_FLAG_AUTOVAR, PROPERTY_FLAG_READ, PROPERTY_FLAG_WRITE,
};

/// Big-endian writers (PEX is big-endian, unlike ESM/BSA).
fn wstring(s: &str) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(s.len() as u16).to_be_bytes());
    out.extend_from_slice(s.as_bytes());
    out
}

fn val_none() -> Vec<u8> {
    vec![0]
}

fn val_ident(index: u16) -> Vec<u8> {
    let mut out = vec![1];
    out.extend_from_slice(&index.to_be_bytes());
    out
}

fn val_str(index: u16) -> Vec<u8> {
    let mut out = vec![2];
    out.extend_from_slice(&index.to_be_bytes());
    out
}

fn val_int(n: i32) -> Vec<u8> {
    let mut out = vec![3];
    out.extend_from_slice(&n.to_be_bytes());
    out
}

fn val_float(f: f32) -> Vec<u8> {
    let mut out = vec![4];
    out.extend_from_slice(&f.to_be_bytes());
    out
}

fn val_bool(b: bool) -> Vec<u8> {
    vec![5, u8::from(b)]
}

fn instr(op: u8, args: &[u8]) -> Vec<u8> {
    let mut out = vec![op];
    out.extend_from_slice(args);
    out
}

fn var_type(name: u16, type_name: u16) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&name.to_be_bytes());
    out.extend_from_slice(&type_name.to_be_bytes());
    out
}

/// A string table builder: interns strings and hands out indices.
struct StrTable {
    strings: Vec<String>,
}

impl StrTable {
    fn new() -> Self {
        // Index 0 is the empty string by convention.
        StrTable {
            strings: vec![String::new()],
        }
    }

    fn intern(&mut self, s: &str) -> u16 {
        if let Some(pos) = self.strings.iter().position(|x| x == s) {
            return pos as u16;
        }
        self.strings.push(s.to_string());
        (self.strings.len() - 1) as u16
    }

    fn bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(self.strings.len() as u16).to_be_bytes());
        for s in &self.strings {
            out.extend_from_slice(&wstring(s));
        }
        out
    }
}

fn function(
    return_type: u16,
    docstring: u16,
    user_flags: u32,
    flags: u8,
    params: (&[u8], u16),
    locals: (&[u8], u16),
    instructions: (&[u8], u16),
) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&return_type.to_be_bytes());
    out.extend_from_slice(&docstring.to_be_bytes());
    out.extend_from_slice(&user_flags.to_be_bytes());
    out.push(flags);
    out.extend_from_slice(&params.1.to_be_bytes());
    out.extend_from_slice(params.0);
    out.extend_from_slice(&locals.1.to_be_bytes());
    out.extend_from_slice(locals.0);
    out.extend_from_slice(&instructions.1.to_be_bytes());
    out.extend_from_slice(instructions.0);
    out
}

fn variable(name: u16, type_name: u16, user_flags: u32, default: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&name.to_be_bytes());
    out.extend_from_slice(&type_name.to_be_bytes());
    out.extend_from_slice(&user_flags.to_be_bytes());
    out.extend_from_slice(default);
    out
}

fn auto_property(name: u16, type_name: u16, docstring: u16, flags: u8, auto_var: u16) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&name.to_be_bytes());
    out.extend_from_slice(&type_name.to_be_bytes());
    out.extend_from_slice(&docstring.to_be_bytes());
    out.extend_from_slice(&0u32.to_be_bytes());
    out.push(flags);
    out.extend_from_slice(&auto_var.to_be_bytes());
    out
}

fn full_property(
    name: u16,
    type_name: u16,
    docstring: u16,
    flags: u8,
    read_handler: Option<&[u8]>,
    write_handler: Option<&[u8]>,
) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&name.to_be_bytes());
    out.extend_from_slice(&type_name.to_be_bytes());
    out.extend_from_slice(&docstring.to_be_bytes());
    out.extend_from_slice(&0u32.to_be_bytes());
    out.push(flags);
    if let Some(handler) = read_handler {
        out.extend_from_slice(handler);
    }
    if let Some(handler) = write_handler {
        out.extend_from_slice(handler);
    }
    out
}

fn named_function(name: u16, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&name.to_be_bytes());
    out.extend_from_slice(body);
    out
}

fn state(name: u16, functions: &[u8], count: u16) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&name.to_be_bytes());
    out.extend_from_slice(&count.to_be_bytes());
    out.extend_from_slice(functions);
    out
}

/// An object: name index + size (which includes the 4 size bytes) +
/// object data.
fn object(name: u16, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&name.to_be_bytes());
    out.extend_from_slice(&(4u32 + data.len() as u32).to_be_bytes());
    out.extend_from_slice(data);
    out
}

fn object_data(
    parent: u16,
    docstring: u16,
    user_flags: u32,
    auto_state: u16,
    variables: (&[u8], u16),
    properties: (&[u8], u16),
    states: (&[u8], u16),
) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&parent.to_be_bytes());
    out.extend_from_slice(&docstring.to_be_bytes());
    out.extend_from_slice(&user_flags.to_be_bytes());
    out.extend_from_slice(&auto_state.to_be_bytes());
    out.extend_from_slice(&variables.1.to_be_bytes());
    out.extend_from_slice(variables.0);
    out.extend_from_slice(&properties.1.to_be_bytes());
    out.extend_from_slice(properties.0);
    out.extend_from_slice(&states.1.to_be_bytes());
    out.extend_from_slice(states.0);
    out
}

/// A minimal but complete PEX file: one empty object, no debug info,
/// no user flags.
fn minimal_pex() -> (Vec<u8>, StrTable) {
    let mut table = StrTable::new();
    let script = table.intern("MyScript");
    let parent = table.intern("ObjectReference");
    let doc = table.intern("A test script");
    let empty = 0u16;

    let data = object_data(parent, doc, 0, empty, (&[], 0), (&[], 0), (&[], 0));
    let obj = object(script, &data);

    let mut out = Vec::new();
    out.extend_from_slice(&MAGIC.to_be_bytes());
    out.push(3); // major
    out.push(1); // minor
    out.extend_from_slice(&1u16.to_be_bytes()); // game ID
    out.extend_from_slice(&1_700_000_000u64.to_be_bytes()); // compilation time
    out.extend_from_slice(&wstring("MyScript.psc"));
    out.extend_from_slice(&wstring("jarvis"));
    out.extend_from_slice(&wstring("workshop"));
    out.extend_from_slice(&table.bytes());
    out.push(0); // no debug info
    out.extend_from_slice(&0u16.to_be_bytes()); // user flags
    out.extend_from_slice(&1u16.to_be_bytes()); // one object
    out.extend_from_slice(&obj);
    (out, table)
}

#[test]
fn parses_minimal_pex() {
    let (bytes, _) = minimal_pex();
    let pex = PexFile::from_bytes(&bytes).unwrap();
    assert_eq!(pex.major_version, 3);
    assert_eq!(pex.minor_version, 1);
    assert_eq!(pex.game_id, 1);
    assert_eq!(pex.compilation_time, 1_700_000_000);
    assert_eq!(pex.source_file_name, "MyScript.psc");
    assert_eq!(pex.username, "jarvis");
    assert_eq!(pex.machine_name, "workshop");
    assert_eq!(pex.debug_info, None);
    assert!(pex.user_flags.is_empty());
    assert_eq!(pex.objects.len(), 1);

    let obj = pex.object("MyScript").unwrap();
    assert_eq!(obj.parent, "ObjectReference");
    assert_eq!(obj.docstring, "A test script");
    assert_eq!(obj.auto_state, "");
    assert!(obj.variables.is_empty());
    assert!(obj.properties.is_empty());
    assert!(obj.states.is_empty());
    assert_eq!(pex.string(1), Some("MyScript"));
    assert_eq!(pex.string(999), None);
}

/// A full object: variables of every value type, auto + full
/// properties, two states, native and global functions, and a
/// function with a varied instruction stream.
fn full_pex() -> Vec<u8> {
    let mut t = StrTable::new();
    let script = t.intern("MyScript");
    let parent = t.intern("ObjectReference");
    let doc = t.intern("A test script");
    let auto_state = t.intern("MyState");
    let empty = 0u16;
    let int_ty = t.intern("Int");
    let float_ty = t.intern("Float");
    let string_ty = t.intern("String");
    let bool_ty = t.intern("Bool");
    let none_ty = t.intern("None");
    let actor_ty = t.intern("Actor");

    // Variables: one of each default-value type.
    let mut vars = Vec::new();
    vars.extend_from_slice(&variable(t.intern("myInt"), int_ty, 0, &val_int(42)));
    vars.extend_from_slice(&variable(t.intern("myFloat"), float_ty, 0, &val_float(1.5)));
    vars.extend_from_slice(&variable(
        t.intern("myString"),
        string_ty,
        0,
        &val_str(t.intern("Hello")),
    ));
    vars.extend_from_slice(&variable(t.intern("myBool"), bool_ty, 0, &val_bool(true)));
    vars.extend_from_slice(&variable(t.intern("myActor"), actor_ty, 0, &val_none()));
    vars.extend_from_slice(&variable(
        t.intern("myIdent"),
        int_ty,
        0,
        &val_ident(t.intern("SomeOtherVar")),
    ));

    // Properties: auto, auto-read-only, full get-only, full get+set.
    let mut props = Vec::new();
    props.extend_from_slice(&auto_property(
        t.intern("AutoProp"),
        float_ty,
        empty,
        PROPERTY_FLAG_READ | PROPERTY_FLAG_WRITE | PROPERTY_FLAG_AUTOVAR,
        t.intern("::AutoProp_var"),
    ));
    props.extend_from_slice(&auto_property(
        t.intern("ReadOnlyProp"),
        string_ty,
        empty,
        PROPERTY_FLAG_READ | PROPERTY_FLAG_AUTOVAR,
        t.intern("::ReadOnlyProp_var"),
    ));
    // Full get-only property: a read handler returning an int.
    let get_body = {
        let mut args = Vec::new();
        args.extend_from_slice(&val_ident(t.intern("myInt")));
        let code = instr(0x1A, &args); // return myInt
        function(int_ty, empty, 0, 0, (&[], 0), (&[], 0), (&code, 1))
    };
    props.extend_from_slice(&full_property(
        t.intern("GetOnlyProp"),
        int_ty,
        empty,
        PROPERTY_FLAG_READ,
        Some(&get_body),
        None,
    ));
    // Full get+set property: read and write handlers.
    let set_body = {
        let mut args = Vec::new();
        args.extend_from_slice(&val_ident(t.intern("myInt")));
        args.extend_from_slice(&val_ident(t.intern("value")));
        let code = instr(0x0D, &args); // assign myInt = value
        let params = var_type(t.intern("value"), int_ty);
        function(none_ty, empty, 0, 0, (&params, 1), (&[], 0), (&code, 1))
    };
    props.extend_from_slice(&full_property(
        t.intern("FullProp"),
        int_ty,
        empty,
        PROPERTY_FLAG_READ | PROPERTY_FLAG_WRITE,
        Some(&get_body),
        Some(&set_body),
    ));

    // A function with a varied instruction stream.
    let a = t.intern("a");
    let b = t.intern("b");
    let mut code = Vec::new();
    // iadd b = a + 1
    let mut args = Vec::new();
    args.extend_from_slice(&val_ident(b));
    args.extend_from_slice(&val_ident(a));
    args.extend_from_slice(&val_int(1));
    code.extend_from_slice(&instr(0x01, &args));
    // callmethod: method Trace on object Debug, result into "result",
    // one vararg: NSS* + count + args (method-first, dest-last — M7.1)
    let mut args = Vec::new();
    args.extend_from_slice(&val_ident(t.intern("Trace")));
    args.extend_from_slice(&val_ident(t.intern("Debug")));
    args.extend_from_slice(&val_ident(t.intern("result")));
    args.extend_from_slice(&val_int(1)); // varargs count
    args.extend_from_slice(&val_str(t.intern("Hello")));
    code.extend_from_slice(&instr(0x17, &args));
    // jmpt b, 2
    let mut args = Vec::new();
    args.extend_from_slice(&val_ident(b));
    args.extend_from_slice(&val_int(2));
    code.extend_from_slice(&instr(0x15, &args));
    // jmp 0
    code.extend_from_slice(&instr(0x14, &val_int(0)));
    // return b
    code.extend_from_slice(&instr(0x1A, &val_ident(b)));
    let params = var_type(a, int_ty);
    let locals = var_type(b, int_ty);
    let do_thing = function(int_ty, empty, 0, 0, (&params, 1), (&locals, 1), (&code, 5));

    // A native function (no bytecode) and a global function.
    let native_fn = function(
        none_ty,
        empty,
        0,
        FUNCTION_FLAG_NATIVE,
        (&[], 0),
        (&[], 0),
        (&[], 0),
    );
    let global_fn = function(
        int_ty,
        empty,
        0,
        FUNCTION_FLAG_GLOBAL,
        (&[], 0),
        (&[], 0),
        (&[], 0),
    );

    // Empty state: DoThing + native + global. MyState: DoThing override.
    let mut empty_fns = Vec::new();
    empty_fns.extend_from_slice(&named_function(t.intern("DoThing"), &do_thing));
    empty_fns.extend_from_slice(&named_function(t.intern("NativeThing"), &native_fn));
    empty_fns.extend_from_slice(&named_function(t.intern("GlobalThing"), &global_fn));
    let empty_state = state(empty, &empty_fns, 3);
    let mut my_state_fns = Vec::new();
    my_state_fns.extend_from_slice(&named_function(t.intern("DoThing"), &do_thing));
    let my_state = state(auto_state, &my_state_fns, 1);
    let mut states = Vec::new();
    states.extend_from_slice(&empty_state);
    states.extend_from_slice(&my_state);

    let data = object_data(
        parent,
        doc,
        0,
        auto_state,
        (&vars, 6),
        (&props, 4),
        (&states, 2),
    );
    let obj = object(script, &data);

    let mut out = Vec::new();
    out.extend_from_slice(&MAGIC.to_be_bytes());
    out.push(3);
    out.push(1);
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&1_700_000_000u64.to_be_bytes());
    out.extend_from_slice(&wstring("MyScript.psc"));
    out.extend_from_slice(&wstring("jarvis"));
    out.extend_from_slice(&wstring("workshop"));
    out.extend_from_slice(&t.bytes());
    out.push(0);
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&obj);
    out
}

#[test]
fn parses_variables_of_every_value_type() {
    let pex = PexFile::from_bytes(&full_pex()).unwrap();
    let obj = pex.object("MyScript").unwrap();
    assert_eq!(obj.variables.len(), 6);
    let int = obj.variable("myInt").unwrap();
    assert_eq!(int.type_name, "Int");
    assert_eq!(int.default, Value::Int(42));
    assert_eq!(obj.variable("myFloat").unwrap().default, Value::Float(1.5));
    assert_eq!(
        obj.variable("myString").unwrap().default,
        Value::Str("Hello".to_string())
    );
    assert_eq!(obj.variable("myBool").unwrap().default, Value::Bool(true));
    assert_eq!(obj.variable("myActor").unwrap().default, Value::None);
    assert_eq!(
        obj.variable("myIdent").unwrap().default,
        Value::Identifier("SomeOtherVar".to_string())
    );
}

#[test]
fn parses_auto_and_full_properties() {
    let pex = PexFile::from_bytes(&full_pex()).unwrap();
    let obj = pex.object("MyScript").unwrap();
    assert_eq!(obj.properties.len(), 4);

    let auto = obj.property("AutoProp").unwrap();
    assert!(auto.is_auto());
    assert!(auto.can_read() && auto.can_write());
    assert_eq!(auto.auto_var.as_deref(), Some("::AutoProp_var"));
    assert!(auto.read_handler.is_none() && auto.write_handler.is_none());

    let ro = obj.property("ReadOnlyProp").unwrap();
    assert!(ro.is_auto());
    assert!(ro.can_read() && !ro.can_write());
    assert_eq!(ro.auto_var.as_deref(), Some("::ReadOnlyProp_var"));

    let get_only = obj.property("GetOnlyProp").unwrap();
    assert!(!get_only.is_auto());
    assert!(get_only.can_read() && !get_only.can_write());
    assert!(get_only.auto_var.is_none());
    let handler = get_only.read_handler.as_ref().unwrap();
    assert_eq!(handler.return_type, "Int");
    assert_eq!(handler.instructions.len(), 1);
    assert_eq!(handler.instructions[0].op, 0x1A);
    assert!(get_only.write_handler.is_none());

    let full = obj.property("FullProp").unwrap();
    assert!(full.can_read() && full.can_write());
    assert!(full.read_handler.is_some());
    let setter = full.write_handler.as_ref().unwrap();
    assert_eq!(setter.params.len(), 1);
    assert_eq!(setter.params[0].name, "value");
    assert_eq!(setter.instructions[0].op, 0x0D);
}

#[test]
fn parses_states_functions_and_instructions() {
    let pex = PexFile::from_bytes(&full_pex()).unwrap();
    let obj = pex.object("MyScript").unwrap();
    assert_eq!(obj.auto_state, "MyState");
    assert_eq!(obj.states.len(), 2);

    let empty = obj.empty_state().unwrap();
    assert!(empty.is_empty_state());
    assert_eq!(empty.functions.len(), 3);

    let do_thing = obj.function("", "DoThing").unwrap();
    assert_eq!(do_thing.return_type, "Int");
    assert_eq!(do_thing.params.len(), 1);
    assert_eq!(do_thing.params[0].name, "a");
    assert_eq!(do_thing.params[0].type_name, "Int");
    assert_eq!(do_thing.locals.len(), 1);
    assert_eq!(do_thing.locals[0].name, "b");
    assert!(!do_thing.is_native() && !do_thing.is_global());
    assert_eq!(do_thing.instructions.len(), 5);

    let expected: Vec<Instruction> = vec![
        Instruction {
            op: 0x01,
            args: vec![
                Value::Identifier("b".to_string()),
                Value::Identifier("a".to_string()),
                Value::Int(1),
            ],
        },
        Instruction {
            op: 0x17,
            args: vec![
                Value::Identifier("Trace".to_string()),
                Value::Identifier("Debug".to_string()),
                Value::Identifier("result".to_string()),
                Value::Str("Hello".to_string()),
            ],
        },
        Instruction {
            op: 0x15,
            args: vec![Value::Identifier("b".to_string()), Value::Int(2)],
        },
        Instruction {
            op: 0x14,
            args: vec![Value::Int(0)],
        },
        Instruction {
            op: 0x1A,
            args: vec![Value::Identifier("b".to_string())],
        },
    ];
    assert_eq!(do_thing.instructions, expected);
    // The varargs count itself is consumed, not kept as an argument.
    assert_eq!(do_thing.instructions[1].args.len(), 4);
    assert_eq!(do_thing.instructions[0].name(), Some("iadd"));
    assert_eq!(do_thing.instructions[1].name(), Some("callmethod"));

    let native = obj.function("", "NativeThing").unwrap();
    assert!(native.is_native());
    assert!(native.instructions.is_empty());

    let global = obj.function("", "GlobalThing").unwrap();
    assert!(global.is_global());

    // The state override resolves through the state lookup.
    assert!(obj.function("MyState", "DoThing").is_some());
    assert!(obj.function("MyState", "NativeThing").is_none());
    assert!(obj.function("", "Nope").is_none());
}

#[test]
fn opcode_names_cover_the_table() {
    assert_eq!(opcode_name(0x00), Some("nop"));
    assert_eq!(opcode_name(0x0D), Some("assign"));
    assert_eq!(opcode_name(0x17), Some("callmethod"));
    assert_eq!(opcode_name(0x18), Some("callparent"));
    assert_eq!(opcode_name(0x19), Some("callstatic"));
    assert_eq!(opcode_name(0x23), Some("array_rfindelement"));
    assert_eq!(opcode_name(0x24), None);
    assert_eq!(opcode_name(0xFF), None);
}

/// A PEX file with debug info and user flags, built on the minimal
/// file by splicing the sections in.
fn debug_pex() -> Vec<u8> {
    let mut t = StrTable::new();
    let script = t.intern("MyScript");
    let parent = t.intern("ObjectReference");
    let doc = t.intern("A test script");
    let empty = 0u16;
    // Intern everything the debug info and user flags reference
    // BEFORE the table is written out.
    let do_thing = t.intern("DoThing");
    let hidden = t.intern("Hidden");
    let conditional = t.intern("Conditional");

    let data = object_data(parent, doc, 0b01, empty, (&[], 0), (&[], 0), (&[], 0));
    let obj = object(script, &data);

    let mut out = Vec::new();
    out.extend_from_slice(&MAGIC.to_be_bytes());
    out.push(3);
    out.push(1);
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&1_700_000_000u64.to_be_bytes());
    out.extend_from_slice(&wstring("MyScript.psc"));
    out.extend_from_slice(&wstring("jarvis"));
    out.extend_from_slice(&wstring("workshop"));
    out.extend_from_slice(&t.bytes());
    // Debug info: present, one function with three line numbers.
    out.push(1);
    out.extend_from_slice(&1_699_000_000u64.to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&script.to_be_bytes());
    out.extend_from_slice(&empty.to_be_bytes());
    out.extend_from_slice(&do_thing.to_be_bytes());
    out.push(0); // function type
    out.extend_from_slice(&3u16.to_be_bytes());
    out.extend_from_slice(&10u16.to_be_bytes());
    out.extend_from_slice(&11u16.to_be_bytes());
    out.extend_from_slice(&12u16.to_be_bytes());
    // User flags: Hidden at bit 0, Conditional at bit 1.
    out.extend_from_slice(&2u16.to_be_bytes());
    out.extend_from_slice(&hidden.to_be_bytes());
    out.push(0);
    out.extend_from_slice(&conditional.to_be_bytes());
    out.push(1);
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&obj);
    out
}

#[test]
fn parses_debug_info_and_user_flags() {
    let pex = PexFile::from_bytes(&debug_pex()).unwrap();
    let debug = pex.debug_info.as_ref().unwrap();
    assert_eq!(debug.modification_time, 1_699_000_000);
    assert_eq!(debug.functions.len(), 1);
    let func = &debug.functions[0];
    assert_eq!(func.object, "MyScript");
    assert_eq!(func.state, "");
    assert_eq!(func.function, "DoThing");
    assert_eq!(func.line_numbers, vec![10, 11, 12]);

    assert_eq!(pex.user_flags.len(), 2);
    assert_eq!(pex.user_flags[0].name, "Hidden");
    assert_eq!(pex.user_flags[0].flag_index, 0);
    assert_eq!(pex.user_flags[1].name, "Conditional");
    assert_eq!(pex.user_flags[1].flag_index, 1);
    // The object has the Hidden bit (0b01) set.
    assert_eq!(pex.objects[0].user_flags, 0b01);
}

#[test]
fn rejects_bad_magic() {
    let (mut bytes, _) = minimal_pex();
    bytes[0] = 0x00;
    assert!(matches!(
        PexFile::from_bytes(&bytes),
        Err(PexError::BadMagic)
    ));
}

#[test]
fn rejects_unsupported_version() {
    let (mut bytes, _) = minimal_pex();
    bytes[4] = 2; // major version 2
    assert!(matches!(
        PexFile::from_bytes(&bytes),
        Err(PexError::UnsupportedVersion(2, 1))
    ));
    let (mut bytes, _) = minimal_pex();
    bytes[5] = 9; // minor version 9
    assert!(matches!(
        PexFile::from_bytes(&bytes),
        Err(PexError::UnsupportedVersion(3, 9))
    ));
}

#[test]
fn rejects_truncated_input() {
    let (bytes, _) = minimal_pex();
    assert!(matches!(
        PexFile::from_bytes(&bytes[..10]),
        Err(PexError::Truncated(_))
    ));
    assert!(matches!(
        PexFile::from_bytes(&[]),
        Err(PexError::Truncated(_))
    ));
    // Cut in the middle of the object data.
    let cut = bytes.len() - 5;
    assert!(matches!(
        PexFile::from_bytes(&bytes[..cut]),
        Err(PexError::Truncated(_))
    ));
}

#[test]
fn rejects_bad_string_index() {
    // Corrupt the object name index to point past the table.
    let (_, table) = minimal_pex();
    // Find the object name index: it sits right after the header +
    // string table + debug flag + user flag count + object count.
    // Rebuild the file with a bad index instead of hunting offsets:
    // the object name is the first u16 after the object count.
    let mut probe = minimal_pex().0;
    // Walk the header by hand: magic(4) + versions(2) + game(2) +
    // time(8) + 3 wstrings + string table + debug(1) + flags(2) +
    // count(2).
    let mut pos = 4 + 2 + 2 + 8;
    for _ in 0..3 {
        let len = u16::from_be_bytes([probe[pos], probe[pos + 1]]) as usize;
        pos += 2 + len;
    }
    let count = u16::from_be_bytes([probe[pos], probe[pos + 1]]) as usize;
    pos += 2;
    for _ in 0..count {
        let len = u16::from_be_bytes([probe[pos], probe[pos + 1]]) as usize;
        pos += 2 + len;
    }
    pos += 1 + 2 + 2; // debug flag, user flag count, object count
    let bad = (table.strings.len() + 5) as u16;
    probe[pos..pos + 2].copy_from_slice(&bad.to_be_bytes());
    assert!(matches!(
        PexFile::from_bytes(&probe),
        Err(PexError::BadStringIndex(_))
    ));
}

#[test]
fn rejects_unknown_opcode() {
    // A function with a single instruction whose opcode is 0xFF.
    let mut t = StrTable::new();
    let script = t.intern("MyScript");
    let parent = t.intern("ObjectReference");
    let empty = 0u16;
    let none_ty = t.intern("None");
    let code = instr(0xFF, &[]);
    let func = function(none_ty, empty, 0, 0, (&[], 0), (&[], 0), (&code, 1));
    let mut fns = Vec::new();
    fns.extend_from_slice(&named_function(t.intern("Boom"), &func));
    let st = state(empty, &fns, 1);
    let data = object_data(parent, empty, 0, empty, (&[], 0), (&[], 0), (&st, 1));
    let obj = object(script, &data);
    let mut out = Vec::new();
    out.extend_from_slice(&MAGIC.to_be_bytes());
    out.push(3);
    out.push(1);
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&0u64.to_be_bytes());
    out.extend_from_slice(&wstring("MyScript.psc"));
    out.extend_from_slice(&wstring(""));
    out.extend_from_slice(&wstring(""));
    out.extend_from_slice(&t.bytes());
    out.push(0);
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&obj);
    assert!(matches!(
        PexFile::from_bytes(&out),
        Err(PexError::UnknownOpcode(0xFF))
    ));
}

#[test]
fn rejects_trailing_bytes_and_bad_object_size() {
    let (mut bytes, _) = minimal_pex();
    bytes.push(0);
    assert!(matches!(
        PexFile::from_bytes(&bytes),
        Err(PexError::InvalidData(_))
    ));

    // Shrink the object size by one: the object data then ends a
    // byte early and the parser finds trailing bytes in the object.
    let (mut bytes, _) = minimal_pex();
    // The object size sits 2 bytes after the object count field;
    // recompute the same offset as in rejects_bad_string_index.
    let mut pos = 4 + 2 + 2 + 8;
    for _ in 0..3 {
        let len = u16::from_be_bytes([bytes[pos], bytes[pos + 1]]) as usize;
        pos += 2 + len;
    }
    let count = u16::from_be_bytes([bytes[pos], bytes[pos + 1]]) as usize;
    pos += 2;
    for _ in 0..count {
        let len = u16::from_be_bytes([bytes[pos], bytes[pos + 1]]) as usize;
        pos += 2 + len;
    }
    pos += 1 + 2 + 2 + 2; // debug, user flags, object count, name index
    let size = u32::from_be_bytes([bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3]]);
    bytes[pos..pos + 4].copy_from_slice(&(size - 1).to_be_bytes());
    assert!(PexFile::from_bytes(&bytes).is_err());
}

/// `Function` is used in assertions through the public API; this
/// keeps the import honest.
#[test]
fn function_type_is_public() {
    let func = Function {
        return_type: "Int".to_string(),
        docstring: String::new(),
        user_flags: 0,
        flags: FUNCTION_FLAG_GLOBAL,
        params: Vec::new(),
        locals: Vec::new(),
        instructions: Vec::new(),
    };
    assert!(func.is_global());
    assert!(!func.is_native());
}
