//! PEX/PSC parser for Dovahengine.
//!
//! Reads Bethesda Papyrus script files (Skyrim Special Edition),
//! following the public UESP format documentation
//! (`docs/formats/pex.md`). This crate contains no Bethesda code and
//! ships no game data: it parses script files the user already owns.
//!
//! Two forms of script:
//! - `.pex` — compiled bytecode, what the game executes. Parsed by
//!   [`PexFile`]: header, string table, debug info, user flags, and
//!   objects (variables, properties, states, functions). Function
//!   bytecode is parsed into instructions (opcode + arguments) but
//!   NOT executed — the runtime is a Phase 3 job (see
//!   `docs/script-runtime-design.md`).
//! - `.psc` — human-readable source, as written in the Creation Kit.
//!   Parsed structurally by the [`psc`] module: script header,
//!   variables, properties, states, and function/event signatures.
//!   Function bodies are not parsed — this is not a compiler.
//!
//! Unlike the ESM and BSA formats, PEX is stored **big-endian**.

pub mod psc;

use std::fs;
use std::io;
use std::path::Path;

/// Magic number at the start of every .pex file ("FASTCODE?").
pub const MAGIC: u32 = 0xFA57_C0DE;

/// Major version of the PEX format Skyrim uses.
pub const VERSION_MAJOR: u8 = 3;

/// Function flag: a global function (no `Self`, callable without an
/// object instance).
pub const FUNCTION_FLAG_GLOBAL: u8 = 0x01;

/// Function flag: a native function — implemented by the engine, so
/// the compiled function carries no bytecode.
pub const FUNCTION_FLAG_NATIVE: u8 = 0x02;

/// Property flag: the property can be read.
pub const PROPERTY_FLAG_READ: u8 = 0x01;

/// Property flag: the property can be written.
pub const PROPERTY_FLAG_WRITE: u8 = 0x02;

/// Property flag: an auto property — the compiler generated a hidden
/// backing variable (`auto_var`) and trivial handlers, so no handler
/// functions are stored.
pub const PROPERTY_FLAG_AUTOVAR: u8 = 0x04;

#[derive(Debug)]
pub enum PexError {
    Io(io::Error),
    /// The first four bytes are not the PEX magic number.
    BadMagic,
    /// A major/minor version this parser does not accept.
    UnsupportedVersion(u8, u8),
    Truncated(&'static str),
    InvalidData(&'static str),
    /// A string-table index that is out of range.
    BadStringIndex(u16),
    /// An instruction opcode with no known argument layout. The
    /// parser cannot resync past it, so the file is rejected.
    UnknownOpcode(u8),
}

impl std::fmt::Display for PexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PexError::Io(e) => write!(f, "I/O error: {e}"),
            PexError::BadMagic => write!(f, "not a PEX file (bad magic)"),
            PexError::UnsupportedVersion(major, minor) => {
                write!(f, "unsupported PEX version {major}.{minor}")
            }
            PexError::Truncated(what) => write!(f, "truncated PEX file in {what}"),
            PexError::InvalidData(what) => write!(f, "invalid PEX data: {what}"),
            PexError::BadStringIndex(index) => {
                write!(f, "string table index {index} out of range")
            }
            PexError::UnknownOpcode(op) => {
                write!(f, "unknown PEX opcode 0x{op:02x}")
            }
        }
    }
}

impl std::error::Error for PexError {}

impl From<io::Error> for PexError {
    fn from(e: io::Error) -> Self {
        PexError::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, PexError>;

/// A Papyrus value, as stored in a `Variable Data` group: a type tag
/// plus its payload. This is the uniform argument type for
/// instructions, and the default-value type for variables.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// Type 0: no value (the Papyrus `none` literal).
    None,
    /// Type 1: an identifier — a name resolved through the string
    /// table (a variable, function, or property name).
    Identifier(String),
    /// Type 2: a string literal, resolved through the string table.
    Str(String),
    /// Type 3: a signed 32-bit integer literal.
    Int(i32),
    /// Type 4: a 32-bit float literal.
    Float(f32),
    /// Type 5: a boolean literal.
    Bool(bool),
}

/// One bytecode instruction: an opcode byte plus its arguments.
/// Arguments are `Value`s; how many an opcode takes is fixed by the
/// opcode table (see [`opcode_args`]), except the three call opcodes,
/// which carry a variable-length argument list.
#[derive(Debug, Clone, PartialEq)]
pub struct Instruction {
    pub op: u8,
    pub args: Vec<Value>,
}

impl Instruction {
    /// The opcode's mnemonic, if it is a known opcode.
    pub fn name(&self) -> Option<&'static str> {
        opcode_name(self.op)
    }
}

/// The mnemonic for an opcode byte, following the UESP opcode table.
/// Returns `None` for unknown opcodes.
pub fn opcode_name(op: u8) -> Option<&'static str> {
    Some(match op {
        0x00 => "nop",
        0x01 => "iadd",
        0x02 => "fadd",
        0x03 => "isub",
        0x04 => "fsub",
        0x05 => "imul",
        0x06 => "fmul",
        0x07 => "idiv",
        0x08 => "fdiv",
        0x09 => "imod",
        0x0A => "not",
        0x0B => "ineg",
        0x0C => "fneg",
        0x0D => "assign",
        0x0E => "cast",
        0x0F => "cmp_eq",
        0x10 => "cmp_lt",
        0x11 => "cmp_le",
        0x12 => "cmp_gt",
        0x13 => "cmp_ge",
        0x14 => "jmp",
        0x15 => "jmpt",
        0x16 => "jmpf",
        0x17 => "callmethod",
        0x18 => "callparent",
        0x19 => "callstatic",
        0x1A => "return",
        0x1B => "strcat",
        0x1C => "propget",
        0x1D => "propset",
        0x1E => "array_create",
        0x1F => "array_length",
        0x20 => "array_getelement",
        0x21 => "array_setelement",
        0x22 => "array_findelement",
        0x23 => "array_rfindelement",
        _ => return None,
    })
}

/// The argument layout of an opcode: the number of fixed `Value`
/// arguments, and whether it carries a variable-length tail (the
/// three call opcodes). Returns `None` for unknown opcodes.
///
/// The UESP table annotates each argument with an expected type
/// letter (S = string/identifier, I = integer, F = float, A = any,
/// L = jump label, ...). Those letters describe the typical type,
/// but the wire format is uniform — every argument is a `Value` —
/// so the parser only needs the count and does not enforce the
/// letters.
fn opcode_args(op: u8) -> Option<(u8, bool)> {
    Some(match op {
        0x00 => (0, false),
        0x01..=0x09 => (3, false),
        0x0A..=0x0E => (2, false),
        0x0F..=0x13 => (3, false),
        0x14 => (1, false),
        0x15 | 0x16 => (2, false),
        0x17 => (3, true), // callmethod NSS*
        0x18 => (2, true), // callparent NS*
        0x19 => (3, true), // callstatic NNS*
        0x1A => (1, false),
        0x1B..=0x1D => (3, false),
        0x1E | 0x1F => (2, false),
        0x20 | 0x21 => (3, false),
        0x22 | 0x23 => (4, false),
        _ => return None,
    })
}

/// A named, typed variable: a function parameter or a local.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VarType {
    pub name: String,
    pub type_name: String,
}

/// A function: return type, parameters, locals, and bytecode. The
/// function itself is nameless in the file — its name lives in the
/// [`NamedFunction`] that wraps it inside a state, or in the
/// property that owns it as a handler.
#[derive(Debug, Clone, PartialEq)]
pub struct Function {
    pub return_type: String,
    pub docstring: String,
    pub user_flags: u32,
    pub flags: u8,
    pub params: Vec<VarType>,
    pub locals: Vec<VarType>,
    pub instructions: Vec<Instruction>,
}

impl Function {
    pub fn is_global(&self) -> bool {
        self.flags & FUNCTION_FLAG_GLOBAL != 0
    }

    pub fn is_native(&self) -> bool {
        self.flags & FUNCTION_FLAG_NATIVE != 0
    }
}

/// A function with its name, as stored inside a state.
#[derive(Debug, Clone, PartialEq)]
pub struct NamedFunction {
    pub name: String,
    pub function: Function,
}

/// A script-level variable with its default value.
#[derive(Debug, Clone, PartialEq)]
pub struct Variable {
    pub name: String,
    pub type_name: String,
    pub user_flags: u32,
    pub default: Value,
}

/// A property: a readable/writable value backed either by a hidden
/// auto variable or by explicit get/set handler functions.
#[derive(Debug, Clone, PartialEq)]
pub struct Property {
    pub name: String,
    pub type_name: String,
    pub docstring: String,
    pub user_flags: u32,
    pub flags: u8,
    /// The hidden backing variable, present for auto properties
    /// (`flags & PROPERTY_FLAG_AUTOVAR != 0`).
    pub auto_var: Option<String>,
    /// The get handler, present for full (non-auto) readable
    /// properties.
    pub read_handler: Option<Function>,
    /// The set handler, present for full (non-auto) writable
    /// properties.
    pub write_handler: Option<Function>,
}

impl Property {
    pub fn can_read(&self) -> bool {
        self.flags & PROPERTY_FLAG_READ != 0
    }

    pub fn can_write(&self) -> bool {
        self.flags & PROPERTY_FLAG_WRITE != 0
    }

    pub fn is_auto(&self) -> bool {
        self.flags & PROPERTY_FLAG_AUTOVAR != 0
    }
}

/// A state: a named mode of a script holding its own versions of
/// functions and events. The empty state (name `""`) holds the
/// default implementations.
#[derive(Debug, Clone, PartialEq)]
pub struct State {
    pub name: String,
    pub functions: Vec<NamedFunction>,
}

impl State {
    /// True for the empty (default) state, whose name is `""`.
    pub fn is_empty_state(&self) -> bool {
        self.name.is_empty()
    }

    pub fn function(&self, name: &str) -> Option<&Function> {
        self.functions
            .iter()
            .find(|f| f.name == name)
            .map(|f| &f.function)
    }
}

/// A compiled script object: one script in the file. A `.pex` file
/// normally holds exactly one object, but the format allows several.
#[derive(Debug, Clone, PartialEq)]
pub struct PexObject {
    pub name: String,
    pub parent: String,
    pub docstring: String,
    pub user_flags: u32,
    /// The auto state — the state the script starts in, or `""` if
    /// it starts in the empty state.
    pub auto_state: String,
    pub variables: Vec<Variable>,
    pub properties: Vec<Property>,
    pub states: Vec<State>,
}

impl PexObject {
    pub fn state(&self, name: &str) -> Option<&State> {
        self.states.iter().find(|s| s.name == name)
    }

    pub fn empty_state(&self) -> Option<&State> {
        self.state("")
    }

    /// Look up a function by state and name.
    pub fn function(&self, state: &str, name: &str) -> Option<&Function> {
        self.state(state).and_then(|s| s.function(name))
    }

    pub fn variable(&self, name: &str) -> Option<&Variable> {
        self.variables.iter().find(|v| v.name == name)
    }

    pub fn property(&self, name: &str) -> Option<&Property> {
        self.properties.iter().find(|p| p.name == name)
    }
}

/// Debug info for one function: which source lines its instructions
/// came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebugFunction {
    pub object: String,
    pub state: String,
    pub function: String,
    /// Function type, valid values 0–3; the exact mapping is not
    /// documented by UESP, so the raw value is kept.
    pub function_type: u8,
    /// One source line number per instruction, in order.
    pub line_numbers: Vec<u16>,
}

/// File-level debug info, present only when the compiler emitted it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebugInfo {
    /// Unix timestamp of the source file's last modification.
    pub modification_time: u64,
    pub functions: Vec<DebugFunction>,
}

/// A user flag definition: a named bit (e.g. `Hidden`,
/// `Conditional`) that objects, variables, properties, and functions
/// reference through their `user_flags` bitmask.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserFlag {
    pub name: String,
    pub flag_index: u8,
}

/// A parsed `.pex` file: one compiled Papyrus script (or, rarely,
/// several objects in one file).
#[derive(Debug, Clone, PartialEq)]
pub struct PexFile {
    pub major_version: u8,
    pub minor_version: u8,
    pub game_id: u16,
    /// Compilation time as a Unix timestamp.
    pub compilation_time: u64,
    pub source_file_name: String,
    pub username: String,
    pub machine_name: String,
    /// The string table: every name, type, and literal in the file
    /// is an index into this table.
    pub strings: Vec<String>,
    pub debug_info: Option<DebugInfo>,
    pub user_flags: Vec<UserFlag>,
    pub objects: Vec<PexObject>,
}

impl PexFile {
    /// Parse a `.pex` file from disk.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let bytes = fs::read(path)?;
        Self::from_bytes(&bytes)
    }

    /// Parse a `.pex` file from its bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(bytes);
        let magic = reader.u32("magic")?;
        if magic != MAGIC {
            return Err(PexError::BadMagic);
        }
        let major_version = reader.u8("major version")?;
        let minor_version = reader.u8("minor version")?;
        if major_version != VERSION_MAJOR || minor_version > 2 {
            return Err(PexError::UnsupportedVersion(major_version, minor_version));
        }
        let game_id = reader.u16("game ID")?;
        let compilation_time = reader.u64("compilation time")?;
        let source_file_name = reader.wstring("source file name")?;
        let username = reader.wstring("username")?;
        let machine_name = reader.wstring("machine name")?;

        let string_count = reader.u16("string table count")? as usize;
        let mut strings = Vec::with_capacity(string_count);
        for _ in 0..string_count {
            strings.push(reader.wstring("string table entry")?);
        }

        let has_debug_info = reader.u8("debug info flag")?;
        let debug_info = if has_debug_info == 0 {
            None
        } else {
            Some(parse_debug_info(&mut reader, &strings)?)
        };

        let user_flag_count = reader.u16("user flag count")? as usize;
        let mut user_flags = Vec::with_capacity(user_flag_count);
        for _ in 0..user_flag_count {
            let name = resolve(&strings, reader.u16("user flag name")?)?;
            let flag_index = reader.u8("user flag index")?;
            user_flags.push(UserFlag { name, flag_index });
        }

        let object_count = reader.u16("object count")? as usize;
        let mut objects = Vec::with_capacity(object_count);
        for _ in 0..object_count {
            objects.push(parse_object(&mut reader, &strings)?);
        }

        if !reader.is_empty() {
            return Err(PexError::InvalidData("trailing bytes after objects"));
        }
        Ok(PexFile {
            major_version,
            minor_version,
            game_id,
            compilation_time,
            source_file_name,
            username,
            machine_name,
            strings,
            debug_info,
            user_flags,
            objects,
        })
    }

    /// Look up a string-table entry by index.
    pub fn string(&self, index: u16) -> Option<&str> {
        self.strings.get(index as usize).map(String::as_str)
    }

    /// Find an object (script) by name.
    pub fn object(&self, name: &str) -> Option<&PexObject> {
        self.objects.iter().find(|o| o.name == name)
    }
}

/// Resolve a string-table index, or fail with [`PexError::BadStringIndex`].
fn resolve(strings: &[String], index: u16) -> Result<String> {
    strings
        .get(index as usize)
        .cloned()
        .ok_or(PexError::BadStringIndex(index))
}

/// A bounds-checked big-endian cursor over the file bytes.
struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Reader { bytes, pos: 0 }
    }

    fn is_empty(&self) -> bool {
        self.pos >= self.bytes.len()
    }

    fn remaining(&self) -> usize {
        self.bytes.len() - self.pos
    }

    fn take(&mut self, len: usize, what: &'static str) -> Result<&'a [u8]> {
        if self.remaining() < len {
            return Err(PexError::Truncated(what));
        }
        let slice = &self.bytes[self.pos..self.pos + len];
        self.pos += len;
        Ok(slice)
    }

    fn u8(&mut self, what: &'static str) -> Result<u8> {
        Ok(self.take(1, what)?[0])
    }

    fn u16(&mut self, what: &'static str) -> Result<u16> {
        Ok(u16::from_be_bytes(self.take(2, what)?.try_into().unwrap()))
    }

    fn u32(&mut self, what: &'static str) -> Result<u32> {
        Ok(u32::from_be_bytes(self.take(4, what)?.try_into().unwrap()))
    }

    fn u64(&mut self, what: &'static str) -> Result<u64> {
        Ok(u64::from_be_bytes(self.take(8, what)?.try_into().unwrap()))
    }

    fn i32(&mut self, what: &'static str) -> Result<i32> {
        Ok(i32::from_be_bytes(self.take(4, what)?.try_into().unwrap()))
    }

    fn f32(&mut self, what: &'static str) -> Result<f32> {
        Ok(f32::from_be_bytes(self.take(4, what)?.try_into().unwrap()))
    }

    /// A `wstring`: a u16 big-endian byte length followed by that
    /// many bytes, no terminator, Windows-1252 encoded.
    fn wstring(&mut self, what: &'static str) -> Result<String> {
        let len = self.u16(what)? as usize;
        let bytes = self.take(len, what)?;
        Ok(decode_windows1252(bytes))
    }
}

/// Decode Windows-1252 bytes into a `String` (the UESP string
/// encoding). Bytes 0x00–0x7F and 0xA0–0xFF map straight to the same
/// Unicode code points; 0x80–0x9F use the Windows-1252 table, with
/// the five undefined positions becoming U+FFFD.
fn decode_windows1252(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| windows1252_char(b)).collect()
}

fn windows1252_char(b: u8) -> char {
    match b {
        0x80 => '\u{20AC}', // €
        0x82 => '\u{201A}', // ‚
        0x83 => '\u{0192}', // ƒ
        0x84 => '\u{201E}', // „
        0x85 => '\u{2026}', // …
        0x86 => '\u{2020}', // †
        0x87 => '\u{2021}', // ‡
        0x88 => '\u{02C6}', // ˆ
        0x89 => '\u{2030}', // ‰
        0x8A => '\u{0160}', // Š
        0x8B => '\u{2039}', // ‹
        0x8C => '\u{0152}', // Œ
        0x8E => '\u{017D}', // Ž
        0x91 => '\u{2018}', // ‘
        0x92 => '\u{2019}', // ’
        0x93 => '\u{201C}', // “
        0x94 => '\u{201D}', // ”
        0x95 => '\u{2022}', // •
        0x96 => '\u{2013}', // –
        0x97 => '\u{2014}', // —
        0x98 => '\u{02DC}', // ˜
        0x99 => '\u{2122}', // ™
        0x9A => '\u{0161}', // š
        0x9B => '\u{203A}', // ›
        0x9C => '\u{0153}', // œ
        0x9E => '\u{017E}', // ž
        0x9F => '\u{0178}', // Ÿ
        0x81 | 0x8D | 0x8F | 0x90 | 0x9D => '\u{FFFD}',
        _ => b as char,
    }
}

fn parse_debug_info(reader: &mut Reader, strings: &[String]) -> Result<DebugInfo> {
    let modification_time = reader.u64("debug modification time")?;
    let function_count = reader.u16("debug function count")? as usize;
    let mut functions = Vec::with_capacity(function_count);
    for _ in 0..function_count {
        let object = resolve(strings, reader.u16("debug object name")?)?;
        let state = resolve(strings, reader.u16("debug state name")?)?;
        let function = resolve(strings, reader.u16("debug function name")?)?;
        let function_type = reader.u8("debug function type")?;
        let instruction_count = reader.u16("debug instruction count")? as usize;
        let mut line_numbers = Vec::with_capacity(instruction_count);
        for _ in 0..instruction_count {
            line_numbers.push(reader.u16("debug line number")?);
        }
        functions.push(DebugFunction {
            object,
            state,
            function,
            function_type,
            line_numbers,
        });
    }
    Ok(DebugInfo {
        modification_time,
        functions,
    })
}

fn parse_object(reader: &mut Reader, strings: &[String]) -> Result<PexObject> {
    let name = resolve(strings, reader.u16("object name")?)?;
    // The size includes the 4-byte size field itself, so the object
    // data that follows is size - 4 bytes. Parse it from a bounded
    // sub-reader so a corrupt object cannot run into the next one.
    let size = reader.u32("object size")? as usize;
    if size < 4 {
        return Err(PexError::InvalidData("object size"));
    }
    let data = reader.take(size - 4, "object data")?;
    let mut obj = Reader::new(data);
    let parent = resolve(strings, obj.u16("object parent")?)?;
    let docstring = resolve(strings, obj.u16("object docstring")?)?;
    let user_flags = obj.u32("object user flags")?;
    let auto_state = resolve(strings, obj.u16("object auto state")?)?;

    let num_variables = obj.u16("object variable count")? as usize;
    let mut variables = Vec::with_capacity(num_variables);
    for _ in 0..num_variables {
        variables.push(parse_variable(&mut obj, strings)?);
    }

    let num_properties = obj.u16("object property count")? as usize;
    let mut properties = Vec::with_capacity(num_properties);
    for _ in 0..num_properties {
        properties.push(parse_property(&mut obj, strings)?);
    }

    let num_states = obj.u16("object state count")? as usize;
    let mut states = Vec::with_capacity(num_states);
    for _ in 0..num_states {
        states.push(parse_state(&mut obj, strings)?);
    }

    if !obj.is_empty() {
        return Err(PexError::InvalidData("trailing bytes in object data"));
    }
    Ok(PexObject {
        name,
        parent,
        docstring,
        user_flags,
        auto_state,
        variables,
        properties,
        states,
    })
}

fn parse_variable(reader: &mut Reader, strings: &[String]) -> Result<Variable> {
    let name = resolve(strings, reader.u16("variable name")?)?;
    let type_name = resolve(strings, reader.u16("variable type")?)?;
    let user_flags = reader.u32("variable user flags")?;
    let default = parse_value(reader, strings)?;
    Ok(Variable {
        name,
        type_name,
        user_flags,
        default,
    })
}

fn parse_property(reader: &mut Reader, strings: &[String]) -> Result<Property> {
    let name = resolve(strings, reader.u16("property name")?)?;
    let type_name = resolve(strings, reader.u16("property type")?)?;
    let docstring = resolve(strings, reader.u16("property docstring")?)?;
    let user_flags = reader.u32("property user flags")?;
    let flags = reader.u8("property flags")?;
    let auto_var = if flags & PROPERTY_FLAG_AUTOVAR != 0 {
        Some(resolve(strings, reader.u16("property auto var")?)?)
    } else {
        None
    };
    // A read handler is stored only for full (non-auto) readable
    // properties: read set, autovar clear. Same logic for write.
    let read_handler = if flags & 0x05 == 0x01 {
        Some(parse_function(reader, strings)?)
    } else {
        None
    };
    let write_handler = if flags & 0x06 == 0x02 {
        Some(parse_function(reader, strings)?)
    } else {
        None
    };
    Ok(Property {
        name,
        type_name,
        docstring,
        user_flags,
        flags,
        auto_var,
        read_handler,
        write_handler,
    })
}

fn parse_state(reader: &mut Reader, strings: &[String]) -> Result<State> {
    let name = resolve(strings, reader.u16("state name")?)?;
    let num_functions = reader.u16("state function count")? as usize;
    let mut functions = Vec::with_capacity(num_functions);
    for _ in 0..num_functions {
        let name = resolve(strings, reader.u16("named function name")?)?;
        let function = parse_function(reader, strings)?;
        functions.push(NamedFunction { name, function });
    }
    Ok(State { name, functions })
}

fn parse_function(reader: &mut Reader, strings: &[String]) -> Result<Function> {
    let return_type = resolve(strings, reader.u16("function return type")?)?;
    let docstring = resolve(strings, reader.u16("function docstring")?)?;
    let user_flags = reader.u32("function user flags")?;
    let flags = reader.u8("function flags")?;
    let num_params = reader.u16("function param count")? as usize;
    let mut params = Vec::with_capacity(num_params);
    for _ in 0..num_params {
        params.push(parse_var_type(reader, strings)?);
    }
    let num_locals = reader.u16("function local count")? as usize;
    let mut locals = Vec::with_capacity(num_locals);
    for _ in 0..num_locals {
        locals.push(parse_var_type(reader, strings)?);
    }
    let num_instructions = reader.u16("function instruction count")? as usize;
    let mut instructions = Vec::with_capacity(num_instructions);
    for _ in 0..num_instructions {
        instructions.push(parse_instruction(reader, strings)?);
    }
    Ok(Function {
        return_type,
        docstring,
        user_flags,
        flags,
        params,
        locals,
        instructions,
    })
}

fn parse_var_type(reader: &mut Reader, strings: &[String]) -> Result<VarType> {
    let name = resolve(strings, reader.u16("var type name")?)?;
    let type_name = resolve(strings, reader.u16("var type")?)?;
    Ok(VarType { name, type_name })
}

fn parse_instruction(reader: &mut Reader, strings: &[String]) -> Result<Instruction> {
    let op = reader.u8("instruction opcode")?;
    let (fixed, varargs) = opcode_args(op).ok_or(PexError::UnknownOpcode(op))?;
    let mut args = Vec::new();
    for _ in 0..fixed {
        args.push(parse_value(reader, strings)?);
    }
    if varargs {
        // The varargs tail is a Variable Data integer holding the
        // count, followed by that many Variable Data arguments.
        let count = match parse_value(reader, strings)? {
            Value::Int(n) => n,
            _ => return Err(PexError::InvalidData("varargs count not an integer")),
        };
        if count < 0 {
            return Err(PexError::InvalidData("negative varargs count"));
        }
        for _ in 0..count {
            args.push(parse_value(reader, strings)?);
        }
    }
    Ok(Instruction { op, args })
}

fn parse_value(reader: &mut Reader, strings: &[String]) -> Result<Value> {
    let tag = reader.u8("value type")?;
    Ok(match tag {
        0 => Value::None,
        1 => Value::Identifier(resolve(strings, reader.u16("identifier index")?)?),
        2 => Value::Str(resolve(strings, reader.u16("string index")?)?),
        3 => Value::Int(reader.i32("integer value")?),
        4 => Value::Float(reader.f32("float value")?),
        5 => Value::Bool(reader.u8("bool value")? != 0),
        _ => return Err(PexError::InvalidData("unknown value type")),
    })
}
