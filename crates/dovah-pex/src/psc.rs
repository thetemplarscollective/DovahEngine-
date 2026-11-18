//! Structural parser for Papyrus source files (`.psc`).
//!
//! Reads a `.psc` file and extracts its structure: the script header,
//! imports, variables, properties (with their kinds and flags),
//! property groups, states, and function/event signatures. Function
//! and event *bodies* are skipped — this is a structural parse, not
//! a compiler, and it does not evaluate expressions or check types.
//!
//! Papyrus is case-insensitive for keywords, so all keyword matching
//! below ignores case. Names and types keep the casing the source
//! used.
//!
//! Grammar reference: the papyrus-lang wiki ("Papyrus Scripts" and
//! "Papyrus Types") and the Creation Kit wiki. Skyrim's Papyrus has
//! no structs — those are a Fallout 4 addition — so a `Struct` block
//! is skipped and not reported, rather than parsed.

use std::fmt;
use std::fs;
use std::path::Path;

/// An error parsing a `.psc` file: the 1-based line it happened on
/// and what went wrong. Line 0 means the error is not tied to a line
/// (for example, the file could not be read).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PscError {
    pub line: usize,
    pub message: String,
}

impl PscError {
    fn new(line: usize, message: impl Into<String>) -> Self {
        PscError {
            line,
            message: message.into(),
        }
    }
}

impl fmt::Display for PscError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.line == 0 {
            write!(f, "{}", self.message)
        } else {
            write!(f, "line {}: {}", self.line, self.message)
        }
    }
}

impl std::error::Error for PscError {}

pub type PscResult<T> = std::result::Result<T, PscError>;

/// One parameter of a function or event: a type, a name, and an
/// optional default value kept as raw source text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PscParam {
    pub name: String,
    pub type_name: String,
    pub default: Option<String>,
}

/// A function signature. The body is not parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PscFunction {
    pub name: String,
    /// `None` for a function with no return type (void).
    pub return_type: Option<String>,
    pub params: Vec<PscParam>,
    pub is_global: bool,
    pub is_native: bool,
    /// Flags such as `DebugOnly` / `BetaOnly`, as written.
    pub flags: Vec<String>,
    pub docstring: Option<String>,
    /// 1-based line of the header in the source.
    pub line: usize,
}

/// An event signature. Remote and custom events carry the object
/// type they listen on (`Event ObjectReference.OnActivate(...)`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PscEvent {
    pub name: String,
    pub object_type: Option<String>,
    pub params: Vec<PscParam>,
    pub is_native: bool,
    pub flags: Vec<String>,
    pub docstring: Option<String>,
    /// 1-based line of the header in the source.
    pub line: usize,
}

/// The three kinds of property.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PscPropertyKind {
    /// `Type Property Name Auto` — a public variable backed by a
    /// hidden compiler-generated variable.
    Auto,
    /// `Type Property Name = value AutoReadOnly` — a public constant.
    AutoReadOnly,
    /// A full property with explicit `Get` and/or `Set` functions
    /// and an `EndProperty`.
    Full,
}

/// A property definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PscProperty {
    pub name: String,
    pub type_name: String,
    pub kind: PscPropertyKind,
    /// Initial value as raw source text (auto properties only).
    pub default: Option<String>,
    /// Flags such as `Conditional`, `Const`, `Hidden`, `Mandatory`.
    pub flags: Vec<String>,
    /// True when a full property defines a `Get` function.
    pub has_getter: bool,
    /// True when a full property defines a `Set` function.
    pub has_setter: bool,
    /// The property group this property sits in, if any.
    pub group: Option<String>,
    pub docstring: Option<String>,
    /// 1-based line of the property definition in the source.
    pub line: usize,
}

/// A script-level variable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PscVariable {
    pub name: String,
    pub type_name: String,
    /// Initial value as raw source text, if one was given.
    pub default: Option<String>,
    /// Flags such as `Conditional`, `Const`.
    pub flags: Vec<String>,
    /// 1-based line of the definition in the source.
    pub line: usize,
}

/// A property group: a named, collapsible section of the property
/// window. The properties themselves carry the group name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PscGroup {
    pub name: String,
    /// Flags such as `Collapsed`, `CollapsedOnRef`, `CollapsedOnBase`.
    pub flags: Vec<String>,
    pub docstring: Option<String>,
    /// 1-based line of the `Group` line in the source.
    pub line: usize,
}

/// A state: a named mode holding its own versions of functions and
/// events. Functions and events defined outside any state live in
/// the empty state (see [`PscScript::functions`] /
/// [`PscScript::events`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PscState {
    pub name: String,
    /// True for an `Auto State` — the script starts in this state.
    pub auto: bool,
    pub functions: Vec<PscFunction>,
    pub events: Vec<PscEvent>,
    pub docstring: Option<String>,
    /// 1-based line of the `State` line in the source.
    pub line: usize,
}

/// A parsed `.psc` file: the structure of one Papyrus script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PscScript {
    pub name: String,
    pub parent: Option<String>,
    /// Script flags such as `Hidden`, `Conditional`, `Native`,
    /// `Const`, `DebugOnly`, `BetaOnly`, `Default`, as written.
    pub flags: Vec<String>,
    pub docstring: Option<String>,
    pub imports: Vec<String>,
    pub variables: Vec<PscVariable>,
    pub properties: Vec<PscProperty>,
    pub groups: Vec<PscGroup>,
    pub states: Vec<PscState>,
    /// Functions in the empty state (defined outside any `State`).
    pub functions: Vec<PscFunction>,
    /// Events in the empty state (defined outside any `State`).
    pub events: Vec<PscEvent>,
    /// Custom event names declared with `CustomEvent`.
    pub custom_events: Vec<String>,
}

impl PscScript {
    /// Parse Papyrus source text.
    pub fn parse(source: &str) -> PscResult<Self> {
        Parser::new(source)?.run()
    }

    /// Parse a `.psc` file from disk.
    pub fn open(path: impl AsRef<Path>) -> PscResult<Self> {
        let path = path.as_ref();
        let source = fs::read_to_string(path)
            .map_err(|e| PscError::new(0, format!("could not read {}: {e}", path.display())))?;
        Self::parse(&source)
    }

    /// Find a state by name.
    pub fn state(&self, name: &str) -> Option<&PscState> {
        self.states.iter().find(|s| s.name == name)
    }

    /// The auto state, if the script declares one.
    pub fn auto_state(&self) -> Option<&PscState> {
        self.states.iter().find(|s| s.auto)
    }

    /// All functions, empty-state ones first, then each state's in
    /// source order.
    pub fn all_functions(&self) -> Vec<&PscFunction> {
        let mut out: Vec<&PscFunction> = self.functions.iter().collect();
        for state in &self.states {
            out.extend(state.functions.iter());
        }
        out
    }

    /// All events, empty-state ones first, then each state's in
    /// source order.
    pub fn all_events(&self) -> Vec<&PscEvent> {
        let mut out: Vec<&PscEvent> = self.events.iter().collect();
        for state in &self.states {
            out.extend(state.events.iter());
        }
        out
    }
}

/// A line after comment stripping and line-continuation joining.
/// Doc comments (`{...}`) are kept as their own lines so they can be
/// attached to the declaration they follow.
#[derive(Debug, Clone)]
enum LogicalLine {
    Code { text: String, line: usize },
    Doc { text: String },
}

/// Split source text into logical lines: strip `;` line comments
/// and `;/ ... /;` block comments, keep `{...}` doc comments as
/// their own lines, and join lines ending in `\` with the next line.
/// Strings are respected throughout — a `;` or `{` inside a string
/// literal is not a comment.
fn logical_lines(source: &str) -> PscResult<Vec<LogicalLine>> {
    let chars: Vec<char> = source.chars().collect();
    let mut lines = Vec::new();
    let mut i = 0;
    let mut line_no = 1;
    let mut buf = String::new();
    let mut buf_start_line = 1;
    let mut in_string = false;

    // Flush the current code buffer as a logical line (if non-blank).
    macro_rules! flush_code {
        () => {
            if !buf.trim().is_empty() {
                lines.push(LogicalLine::Code {
                    text: buf.trim().to_string(),
                    line: buf_start_line,
                });
            }
            buf.clear();
        };
    }

    while i < chars.len() {
        let c = chars[i];
        if in_string {
            buf.push(c);
            if c == '\\' {
                // Escape sequence: copy the next character verbatim.
                if i + 1 < chars.len() {
                    buf.push(chars[i + 1]);
                    i += 2;
                    continue;
                }
            } else if c == '"' {
                in_string = false;
            } else if c == '\n' {
                return Err(PscError::new(line_no, "unterminated string literal"));
            }
            i += 1;
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                buf.push(c);
                i += 1;
            }
            ';' => {
                if chars.get(i + 1) == Some(&'/') {
                    // Block comment: consume through /;
                    let start = line_no;
                    i += 2;
                    let mut closed = false;
                    while i < chars.len() {
                        if chars[i] == '\n' {
                            line_no += 1;
                        }
                        if chars[i] == '/' && chars.get(i + 1) == Some(&';') {
                            i += 2;
                            closed = true;
                            break;
                        }
                        i += 1;
                    }
                    if !closed {
                        return Err(PscError::new(start, "unterminated block comment"));
                    }
                } else {
                    // Line comment: skip to the newline.
                    while i < chars.len() && chars[i] != '\n' {
                        i += 1;
                    }
                }
            }
            '{' => {
                // Doc comment: capture through } (may span lines).
                let start = line_no;
                i += 1;
                let mut doc = String::new();
                let mut closed = false;
                while i < chars.len() {
                    if chars[i] == '\n' {
                        line_no += 1;
                    }
                    if chars[i] == '}' {
                        i += 1;
                        closed = true;
                        break;
                    }
                    doc.push(chars[i]);
                    i += 1;
                }
                if !closed {
                    return Err(PscError::new(start, "unterminated documentation comment"));
                }
                flush_code!();
                lines.push(LogicalLine::Doc {
                    text: doc.trim().to_string(),
                });
                buf_start_line = line_no;
            }
            '\n' => {
                let trimmed = buf.trim_end();
                if let Some(without) = trimmed.strip_suffix('\\') {
                    // Line continuation: drop the backslash, keep
                    // accumulating into the same logical line.
                    buf = without.to_string();
                    buf.push(' ');
                } else {
                    flush_code!();
                    buf_start_line = line_no + 1;
                }
                line_no += 1;
                i += 1;
            }
            '\r' => {
                // Tolerate CRLF line endings.
                i += 1;
            }
            _ => {
                buf.push(c);
                i += 1;
            }
        }
    }
    if in_string {
        return Err(PscError::new(line_no, "unterminated string literal"));
    }
    flush_code!();
    Ok(lines)
}

struct Parser {
    lines: Vec<LogicalLine>,
    pos: usize,
    script: PscScript,
    state: Option<PscState>,
    group: Option<PscGroup>,
    property: Option<PscProperty>,
    /// Line number where the currently skipped function body opened.
    in_function: Option<usize>,
    /// Line number where the currently skipped event body opened.
    in_event: Option<usize>,
    /// Line number where the currently skipped struct block opened.
    in_struct: Option<usize>,
    seen_header: bool,
}

impl Parser {
    fn new(source: &str) -> PscResult<Self> {
        Ok(Parser {
            lines: logical_lines(source)?,
            pos: 0,
            script: PscScript {
                name: String::new(),
                parent: None,
                flags: Vec::new(),
                docstring: None,
                imports: Vec::new(),
                variables: Vec::new(),
                properties: Vec::new(),
                groups: Vec::new(),
                states: Vec::new(),
                functions: Vec::new(),
                events: Vec::new(),
                custom_events: Vec::new(),
            },
            state: None,
            group: None,
            property: None,
            in_function: None,
            in_event: None,
            in_struct: None,
            seen_header: false,
        })
    }

    /// If the next logical line is a doc comment, consume it and
    /// return its text. Doc comments follow the declaration they
    /// document, so this is called right after parsing one.
    fn take_following_doc(&mut self) -> Option<String> {
        if let Some(LogicalLine::Doc { text, .. }) = self.lines.get(self.pos) {
            let text = text.clone();
            self.pos += 1;
            Some(text)
        } else {
            None
        }
    }

    fn run(mut self) -> PscResult<PscScript> {
        while self.pos < self.lines.len() {
            match self.lines[self.pos].clone() {
                LogicalLine::Doc { .. } => {
                    // A doc comment with no declaration before it:
                    // nothing to attach to, so it is ignored.
                    self.pos += 1;
                }
                LogicalLine::Code { text, line } => {
                    self.pos += 1;
                    self.handle_code_line(&text, line)?;
                }
            }
        }
        if let Some(open) = self.in_function {
            return Err(PscError::new(open, "unterminated function"));
        }
        if let Some(open) = self.in_event {
            return Err(PscError::new(open, "unterminated event"));
        }
        if let Some(open) = self.in_struct {
            return Err(PscError::new(open, "unterminated struct"));
        }
        if let Some(prop) = &self.property {
            return Err(PscError::new(prop.line, "unterminated property"));
        }
        if let Some(group) = &self.group {
            return Err(PscError::new(group.line, "unterminated group"));
        }
        if let Some(state) = &self.state {
            return Err(PscError::new(state.line, "unterminated state"));
        }
        if !self.seen_header {
            return Err(PscError::new(0, "missing ScriptName header"));
        }
        Ok(self.script)
    }

    fn handle_code_line(&mut self, text: &str, line: usize) -> PscResult<()> {
        // Body-skipping modes take precedence: inside a function,
        // event, or struct, only the terminator matters.
        if self.in_function.is_some() {
            if eq_ci(text, "EndFunction") {
                self.in_function = None;
            }
            return Ok(());
        }
        if self.in_event.is_some() {
            if eq_ci(text, "EndEvent") {
                self.in_event = None;
            }
            return Ok(());
        }
        if self.in_struct.is_some() {
            if eq_ci(text, "EndStruct") {
                self.in_struct = None;
            }
            return Ok(());
        }

        // Inside a full property: only Get/Set functions and
        // EndProperty are allowed.
        if self.property.is_some() {
            if eq_ci(text, "EndProperty") {
                let prop = self.property.take().unwrap();
                self.script.properties.push(prop);
                return Ok(());
            }
            if let Some(func) = parse_function_header(text, line)? {
                let prop = self.property.as_mut().unwrap();
                match func.name.to_ascii_lowercase().as_str() {
                    "get" => prop.has_getter = true,
                    "set" => prop.has_setter = true,
                    _ => {
                        return Err(PscError::new(
                            line,
                            "only Get and Set functions may appear in a property",
                        ));
                    }
                }
                if !func.is_native {
                    self.in_function = Some(line);
                }
                return Ok(());
            }
            return Err(PscError::new(
                line,
                "unexpected line inside a property definition",
            ));
        }

        // Inside a group: only properties and EndGroup.
        if self.group.is_some() {
            if eq_ci(text, "EndGroup") {
                let group = self.group.take().unwrap();
                self.script.groups.push(group);
                return Ok(());
            }
            if let Some(prop) = self.try_parse_property(text, line)? {
                self.finish_property(prop);
                return Ok(());
            }
            return Err(PscError::new(line, "only properties may appear in a group"));
        }

        // Inside a state: only functions, events, and EndState.
        if self.state.is_some() {
            if eq_ci(text, "EndState") {
                let state = self.state.take().unwrap();
                self.script.states.push(state);
                return Ok(());
            }
            if let Some(mut func) = parse_function_header(text, line)? {
                func.docstring = self.take_following_doc();
                if !func.is_native {
                    self.in_function = Some(line);
                }
                self.state.as_mut().unwrap().functions.push(func);
                return Ok(());
            }
            if let Some(mut event) = parse_event_header(text, line)? {
                event.docstring = self.take_following_doc();
                if !event.is_native {
                    self.in_event = Some(line);
                }
                self.state.as_mut().unwrap().events.push(event);
                return Ok(());
            }
            return Err(PscError::new(
                line,
                "only functions and events may appear in a state",
            ));
        }

        self.handle_top_level(text, line)
    }

    /// A property that was just parsed: auto properties are complete
    /// at once; full properties open a block that collects Get/Set
    /// until EndProperty.
    fn finish_property(&mut self, mut prop: PscProperty) {
        prop.docstring = self.take_following_doc();
        prop.group = self.group.as_ref().map(|g| g.name.clone());
        if prop.kind == PscPropertyKind::Full {
            self.property = Some(prop);
        } else {
            self.script.properties.push(prop);
        }
    }

    fn try_parse_property(&mut self, text: &str, line: usize) -> PscResult<Option<PscProperty>> {
        parse_property(text, line)
    }

    fn handle_top_level(&mut self, text: &str, line: usize) -> PscResult<()> {
        let words = split_words(text);
        if words.is_empty() {
            return Ok(());
        }
        let first = words[0].to_ascii_lowercase();

        if first == "scriptname" {
            if self.seen_header {
                return Err(PscError::new(line, "ScriptName must be the first line"));
            }
            self.seen_header = true;
            self.parse_header(&words, line)?;
            self.script.docstring = self.take_following_doc();
            return Ok(());
        }
        if !self.seen_header {
            return Err(PscError::new(line, "first line must be ScriptName"));
        }

        match first.as_str() {
            "import" => {
                if words.len() != 2 || !is_identifier(&words[1]) {
                    return Err(PscError::new(line, "malformed Import"));
                }
                self.script.imports.push(words[1].clone());
                return Ok(());
            }
            "customevent" => {
                if words.len() != 2 || !is_identifier(&words[1]) {
                    return Err(PscError::new(line, "malformed CustomEvent"));
                }
                self.script.custom_events.push(words[1].clone());
                return Ok(());
            }
            "group" => {
                if words.len() < 2 || !is_identifier(&words[1]) {
                    return Err(PscError::new(line, "malformed Group"));
                }
                let mut group = PscGroup {
                    name: words[1].clone(),
                    flags: words[2..].to_vec(),
                    docstring: None,
                    line,
                };
                group.docstring = self.take_following_doc();
                self.group = Some(group);
                return Ok(());
            }
            "state" => {
                if words.len() != 2 || !is_identifier(&words[1]) {
                    return Err(PscError::new(line, "malformed State"));
                }
                let mut state = PscState {
                    name: words[1].clone(),
                    auto: false,
                    functions: Vec::new(),
                    events: Vec::new(),
                    docstring: None,
                    line,
                };
                state.docstring = self.take_following_doc();
                self.state = Some(state);
                return Ok(());
            }
            "auto" => {
                if words.len() == 3 && eq_ci(&words[1], "State") && is_identifier(&words[2]) {
                    let mut state = PscState {
                        name: words[2].clone(),
                        auto: true,
                        functions: Vec::new(),
                        events: Vec::new(),
                        docstring: None,
                        line,
                    };
                    state.docstring = self.take_following_doc();
                    self.state = Some(state);
                    return Ok(());
                }
                return Err(PscError::new(line, "malformed Auto State"));
            }
            "struct" => {
                // Fallout 4 only; tolerated and skipped, not parsed.
                self.in_struct = Some(line);
                return Ok(());
            }
            "endgroup" | "endstate" | "endproperty" | "endfunction" | "endevent" | "endstruct" => {
                return Err(PscError::new(
                    line,
                    format!("{text} without a matching block"),
                ));
            }
            _ => {}
        }

        if let Some(mut func) = parse_function_header(text, line)? {
            func.docstring = self.take_following_doc();
            if !func.is_native {
                self.in_function = Some(line);
            }
            self.script.functions.push(func);
            return Ok(());
        }
        if let Some(mut event) = parse_event_header(text, line)? {
            event.docstring = self.take_following_doc();
            if !event.is_native {
                self.in_event = Some(line);
            }
            self.script.events.push(event);
            return Ok(());
        }
        if let Some(prop) = parse_property(text, line)? {
            self.finish_property(prop);
            return Ok(());
        }
        if let Some(var) = parse_variable(text, line)? {
            self.script.variables.push(var);
            return Ok(());
        }
        Err(PscError::new(line, format!("unrecognized line: {text}")))
    }

    /// `ScriptName <name> [Extends <parent>] [flags...]`
    fn parse_header(&mut self, words: &[String], line: usize) -> PscResult<()> {
        if words.len() < 2 || !is_identifier(&words[1]) {
            return Err(PscError::new(line, "malformed ScriptName"));
        }
        self.script.name = words[1].clone();
        let mut i = 2;
        if i < words.len() && eq_ci(&words[i], "Extends") {
            if i + 1 >= words.len() || !is_identifier(&words[i + 1]) {
                return Err(PscError::new(line, "malformed Extends"));
            }
            self.script.parent = Some(words[i + 1].clone());
            i += 2;
        }
        for flag in &words[i..] {
            if !is_known_script_flag(flag) {
                return Err(PscError::new(line, format!("unknown script flag: {flag}")));
            }
            self.script.flags.push(flag.clone());
        }
        Ok(())
    }
}

/// `ScriptName` flags from the papyrus-lang wiki.
fn is_known_script_flag(word: &str) -> bool {
    matches!(
        word.to_ascii_lowercase().as_str(),
        "conditional" | "const" | "debugonly" | "betaonly" | "hidden" | "native" | "default"
    )
}

/// Property flags from the papyrus-lang wiki.
fn is_known_property_flag(word: &str) -> bool {
    matches!(
        word.to_ascii_lowercase().as_str(),
        "conditional" | "const" | "hidden" | "mandatory"
    )
}

/// Script-variable flags from the papyrus-lang wiki.
fn is_known_variable_flag(word: &str) -> bool {
    matches!(word.to_ascii_lowercase().as_str(), "conditional" | "const")
}

/// Case-insensitive word equality.
fn eq_ci(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// A Papyrus identifier: letters, digits, and underscores, not
/// starting with a digit, optionally namespaced with `:` (e.g.
/// `MyMod:Utility`).
fn is_identifier(word: &str) -> bool {
    if word.is_empty() {
        return false;
    }
    word.split(':').all(|segment| {
        let mut chars = segment.chars();
        match chars.next() {
            Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
            _ => return false,
        }
        chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

/// A Papyrus type: an identifier, optionally with an `[]` array
/// suffix (e.g. `Int[]`, `Actor[]`).
fn is_type(word: &str) -> bool {
    let base = word.strip_suffix("[]").unwrap_or(word);
    is_identifier(base)
}

/// A Papyrus type that may actually appear in a declaration: a
/// primitive type, or an identifier that is not a reserved word.
/// This keeps a misplaced keyword (e.g. `Native Function Foo()`)
/// from being read as a return type.
fn is_valid_type(word: &str) -> bool {
    if !is_type(word) {
        return false;
    }
    let base = word.strip_suffix("[]").unwrap_or(word);
    is_primitive_type(base) || !is_reserved(base)
}

/// The built-in Papyrus types.
fn is_primitive_type(word: &str) -> bool {
    matches!(
        word.to_ascii_lowercase().as_str(),
        "int" | "float" | "bool" | "string" | "var"
    )
}

/// Papyrus reserved words: these may not be used as identifiers.
fn is_reserved(word: &str) -> bool {
    matches!(
        word.to_ascii_lowercase().as_str(),
        "scriptname"
            | "extends"
            | "import"
            | "customevent"
            | "group"
            | "endgroup"
            | "state"
            | "endstate"
            | "auto"
            | "property"
            | "endproperty"
            | "function"
            | "endfunction"
            | "event"
            | "endevent"
            | "struct"
            | "endstruct"
            | "native"
            | "global"
            | "const"
            | "conditional"
            | "hidden"
            | "mandatory"
            | "debugonly"
            | "betaonly"
            | "default"
            | "return"
            | "if"
            | "elseif"
            | "else"
            | "endif"
            | "while"
            | "endwhile"
            | "bool"
            | "int"
            | "float"
            | "string"
            | "var"
            | "none"
            | "true"
            | "false"
            | "self"
            | "parent"
            | "new"
            | "as"
            | "is"
    )
}

/// Split a line into words on whitespace, keeping double-quoted
/// strings (with escapes) as single words, quotes included.
fn split_words(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut in_string = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if in_string {
            current.push(c);
            if c == '\\' {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            } else if c == '"' {
                in_string = false;
            }
        } else if c == '"' {
            in_string = true;
            current.push(c);
        } else if c.is_whitespace() {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
        } else {
            current.push(c);
        }
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

/// Split a parameter list on top-level commas: commas inside strings
/// or nested `()` / `[]` do not split.
fn split_params(inner: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut in_string = false;
    let mut depth = 0i32;
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        if in_string {
            current.push(c);
            if c == '\\' {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            } else if c == '"' {
                in_string = false;
            }
        } else {
            match c {
                '"' => {
                    in_string = true;
                    current.push(c);
                }
                '(' | '[' => {
                    depth += 1;
                    current.push(c);
                }
                ')' | ']' => {
                    depth -= 1;
                    current.push(c);
                }
                ',' if depth == 0 => {
                    parts.push(std::mem::take(&mut current));
                }
                _ => current.push(c),
            }
        }
    }
    if !current.trim().is_empty() || !parts.is_empty() {
        parts.push(current);
    }
    parts
}

/// Find the matching close paren for the `(` at `open`, respecting
/// string literals. Returns the byte index of the `)`.
fn matching_paren(text: &str, open: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0i32;
    let mut in_string = false;
    let mut i = open;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if in_string {
            if c == '\\' {
                i += 2;
                continue;
            }
            if c == '"' {
                in_string = false;
            }
        } else {
            match c {
                '"' => in_string = true,
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    None
}

/// Parse `<type> <name> [= <default>]` for one parameter.
fn parse_param(part: &str, line: usize) -> PscResult<PscParam> {
    let words = split_words(part);
    if words.len() < 2
        || !is_valid_type(&words[0])
        || !is_identifier(&words[1])
        || is_reserved(&words[1])
    {
        return Err(PscError::new(line, format!("malformed parameter: {part}")));
    }
    let mut default = None;
    if words.len() > 2 {
        if words[2] != "=" || words.len() < 4 {
            return Err(PscError::new(line, format!("malformed parameter: {part}")));
        }
        default = Some(words[3..].join(" "));
    }
    Ok(PscParam {
        name: words[1].clone(),
        type_name: words[0].clone(),
        default,
    })
}

/// Parse the text inside a function's parentheses into parameters.
fn parse_params(inner: &str, line: usize) -> PscResult<Vec<PscParam>> {
    if inner.trim().is_empty() {
        return Ok(Vec::new());
    }
    split_params(inner)
        .iter()
        .map(|part| parse_param(part.trim(), line))
        .collect()
}

/// Parse a function header: `[<type>] Function <name>(<params>)
/// [Global] [Native] [flags...]`. Returns `None` when the line is
/// not a function header at all (no `Function` word); returns an
/// error when it looks like a function header but is malformed.
fn parse_function_header(text: &str, line: usize) -> PscResult<Option<PscFunction>> {
    let open = match text.find('(') {
        Some(i) => i,
        None => {
            if split_words(text).iter().any(|w| eq_ci(w, "Function")) {
                return Err(PscError::new(line, "malformed function header"));
            }
            return Ok(None);
        }
    };
    let head = &text[..open];
    let head_words = split_words(head);
    let fn_pos = match head_words.iter().position(|w| eq_ci(w, "Function")) {
        Some(p) => p,
        None => return Ok(None),
    };
    if fn_pos > 1 || head_words.len() != fn_pos + 2 {
        return Err(PscError::new(line, "malformed function header"));
    }
    let return_type = if fn_pos == 1 {
        if !is_valid_type(&head_words[0]) {
            return Err(PscError::new(line, "malformed function return type"));
        }
        Some(head_words[0].clone())
    } else {
        None
    };
    let name = head_words[fn_pos + 1].clone();
    if !is_identifier(&name) || is_reserved(&name) {
        return Err(PscError::new(line, "malformed function name"));
    }
    let close = matching_paren(text, open)
        .ok_or_else(|| PscError::new(line, "unbalanced parentheses in function header"))?;
    let params = parse_params(&text[open + 1..close], line)?;
    let mut is_global = false;
    let mut is_native = false;
    let mut flags = Vec::new();
    for word in split_words(&text[close + 1..]) {
        match word.to_ascii_lowercase().as_str() {
            "global" => is_global = true,
            "native" => is_native = true,
            "debugonly" | "betaonly" => flags.push(word),
            _ => {
                return Err(PscError::new(
                    line,
                    format!("unknown function modifier: {word}"),
                ));
            }
        }
    }
    Ok(Some(PscFunction {
        name,
        return_type,
        params,
        is_global,
        is_native,
        flags,
        docstring: None,
        line,
    }))
}

/// Parse an event header: `Event <name>(<params>) [Native]
/// [flags...]`, or a remote/custom event `Event
/// <ObjectType>.<name>(<sender>, <params>)`. Returns `None` when the
/// line does not start with `Event`.
fn parse_event_header(text: &str, line: usize) -> PscResult<Option<PscEvent>> {
    let open = match text.find('(') {
        Some(i) => i,
        None => return Ok(None),
    };
    let head = text[..open].trim();
    let head_words = split_words(head);
    if head_words.len() != 2 || !eq_ci(&head_words[0], "Event") {
        return Ok(None);
    }
    let dotted = &head_words[1];
    let (object_type, name) = match dotted.split_once('.') {
        Some((obj, name)) => (Some(obj.to_string()), name.to_string()),
        None => (None, dotted.clone()),
    };
    if !is_identifier(&name) || is_reserved(&name) {
        return Err(PscError::new(line, "malformed event name"));
    }
    if let Some(obj) = &object_type {
        if !is_identifier(obj) {
            return Err(PscError::new(line, "malformed event object type"));
        }
    }
    let close = matching_paren(text, open)
        .ok_or_else(|| PscError::new(line, "unbalanced parentheses in event header"))?;
    let params = parse_params(&text[open + 1..close], line)?;
    let mut is_native = false;
    let mut flags = Vec::new();
    for word in split_words(&text[close + 1..]) {
        match word.to_ascii_lowercase().as_str() {
            "native" => is_native = true,
            "debugonly" | "betaonly" => flags.push(word),
            _ => {
                return Err(PscError::new(
                    line,
                    format!("unknown event modifier: {word}"),
                ));
            }
        }
    }
    Ok(Some(PscEvent {
        name,
        object_type,
        params,
        is_native,
        flags,
        docstring: None,
        line,
    }))
}

/// Parse a property definition. Returns `None` when the line is not
/// a property definition (no `Property` word as the second word).
fn parse_property(text: &str, line: usize) -> PscResult<Option<PscProperty>> {
    let words = split_words(text);
    let prop_pos = match words.iter().position(|w| eq_ci(w, "Property")) {
        Some(p) => p,
        None => return Ok(None),
    };
    if prop_pos != 1 || words.len() < 3 {
        return Err(PscError::new(line, "malformed property"));
    }
    if !is_valid_type(&words[0]) {
        return Err(PscError::new(line, "malformed property type"));
    }
    let name = words[2].clone();
    if !is_identifier(&name) || is_reserved(&name) {
        return Err(PscError::new(line, "malformed property name"));
    }
    // Remainder: optional `= <default>`, then Auto / AutoReadOnly
    // (or neither, for a full property), then flags.
    let rest = &words[3..];
    let mut i = 0;
    let mut default = None;
    if i < rest.len() && rest[i] == "=" {
        i += 1;
        let mut parts = Vec::new();
        while i < rest.len()
            && !eq_ci(&rest[i], "Auto")
            && !eq_ci(&rest[i], "AutoReadOnly")
            && !is_known_property_flag(&rest[i])
        {
            parts.push(rest[i].clone());
            i += 1;
        }
        if parts.is_empty() {
            return Err(PscError::new(line, "property default value missing"));
        }
        default = Some(parts.join(" "));
    }
    let mut kind = PscPropertyKind::Full;
    if i < rest.len() && eq_ci(&rest[i], "Auto") {
        kind = PscPropertyKind::Auto;
        i += 1;
    } else if i < rest.len() && eq_ci(&rest[i], "AutoReadOnly") {
        kind = PscPropertyKind::AutoReadOnly;
        i += 1;
        if default.is_none() {
            return Err(PscError::new(
                line,
                "AutoReadOnly property requires a value",
            ));
        }
    }
    let mut flags = Vec::new();
    for flag in &rest[i..] {
        if !is_known_property_flag(flag) {
            return Err(PscError::new(
                line,
                format!("unknown property flag: {flag}"),
            ));
        }
        flags.push(flag.clone());
    }
    Ok(Some(PscProperty {
        name,
        type_name: words[0].clone(),
        kind,
        default,
        flags,
        has_getter: false,
        has_setter: false,
        group: None,
        docstring: None,
        line,
    }))
}

/// Parse a script-level variable: `<type> <name> [= <default>]
/// [flags...]`. Returns `None` when the line is not a variable
/// definition at all.
fn parse_variable(text: &str, line: usize) -> PscResult<Option<PscVariable>> {
    let words = split_words(text);
    if words.len() < 2
        || !is_valid_type(&words[0])
        || !is_identifier(&words[1])
        || is_reserved(&words[1])
    {
        return Ok(None);
    }
    let rest = &words[2..];
    let mut i = 0;
    let mut default = None;
    if i < rest.len() && rest[i] == "=" {
        i += 1;
        let mut parts = Vec::new();
        while i < rest.len() && !is_known_variable_flag(&rest[i]) {
            parts.push(rest[i].clone());
            i += 1;
        }
        if parts.is_empty() {
            return Err(PscError::new(line, "variable default value missing"));
        }
        default = Some(parts.join(" "));
    }
    let mut flags = Vec::new();
    for flag in &rest[i..] {
        if !is_known_variable_flag(flag) {
            return Err(PscError::new(
                line,
                format!("unknown variable flag: {flag}"),
            ));
        }
        flags.push(flag.clone());
    }
    Ok(Some(PscVariable {
        name: words[1].clone(),
        type_name: words[0].clone(),
        default,
        flags,
        line,
    }))
}
