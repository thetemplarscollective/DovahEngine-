//! Tests for the PSC structural parser, using synthetic Papyrus
//! source written in the tests. No real game data is involved.

use dovah_pex::psc::{PscPropertyKind, PscScript};

const FULL_SCRIPT: &str = r#"; A comment line
ScriptName MyScript Extends ObjectReference Hidden
{The docstring for the whole script.}

Import Utility
Import MyNamespace:Helpers

int myInt = 42
float myFloat
string myText = "Hello, world!" Const
Actor[] myActors
bool myFlag Conditional

CustomEvent MyCustomEvent

Group Display Collapsed
  string Property Title = "Untitled" Auto
  int Property Count Auto Hidden
EndGroup

float Property Speed = 1.0 Auto
string Property Greeting = "Hello world!" AutoReadOnly

int Property FullValue
  {A full property with a getter and a setter.}
  Function Set(int newValue)
    myInt = newValue
  EndFunction
  int Function Get()
    return myInt
  EndFunction
EndProperty

int Property ReadOnlyValue
  int Function Get()
    return myInt
  EndFunction
EndProperty

Function DoNothing()
  ; nothing here
EndFunction

int Function AddTwo(int a, int b) Global
  return a + b
EndFunction

Function WithDefaults(int howMuch = 1, string label = "x,y")
  Debug.Trace(label)
EndFunction

Function NativeThing() Native

Event OnActivate(ObjectReference akActivator)
  Debug.Trace("activated")
EndEvent

Event ObjectReference.OnActivate(ObjectReference akSender, ObjectReference akActivator)
  Debug.Trace("remote")
EndEvent

State MyState
  int Function AddTwo(int a, int b)
    return a - b
  EndFunction
  Event OnActivate(ObjectReference akActivator)
  EndEvent
EndState

Auto State Starting
  Function DoNothing()
  EndFunction
EndState
"#;

#[test]
fn parses_full_script_header_and_imports() {
    let script = PscScript::parse(FULL_SCRIPT).unwrap();
    assert_eq!(script.name, "MyScript");
    assert_eq!(script.parent.as_deref(), Some("ObjectReference"));
    assert_eq!(script.flags, vec!["Hidden"]);
    assert_eq!(
        script.docstring.as_deref(),
        Some("The docstring for the whole script.")
    );
    assert_eq!(script.imports, vec!["Utility", "MyNamespace:Helpers"]);
    assert_eq!(script.custom_events, vec!["MyCustomEvent"]);
}

#[test]
fn parses_variables_with_defaults_and_flags() {
    let script = PscScript::parse(FULL_SCRIPT).unwrap();
    assert_eq!(script.variables.len(), 5);
    let int = &script.variables[0];
    assert_eq!(int.name, "myInt");
    assert_eq!(int.type_name, "int");
    assert_eq!(int.default.as_deref(), Some("42"));
    assert!(int.flags.is_empty());
    let text = &script.variables[2];
    assert_eq!(text.name, "myText");
    assert_eq!(text.default.as_deref(), Some("\"Hello, world!\""));
    assert_eq!(text.flags, vec!["Const"]);
    let actors = &script.variables[3];
    assert_eq!(actors.type_name, "Actor[]");
    assert_eq!(actors.default, None);
    let flag = &script.variables[4];
    assert_eq!(flag.flags, vec!["Conditional"]);
}

#[test]
fn parses_properties_groups_and_kinds() {
    let script = PscScript::parse(FULL_SCRIPT).unwrap();
    assert_eq!(script.groups.len(), 1);
    assert_eq!(script.groups[0].name, "Display");
    assert_eq!(script.groups[0].flags, vec!["Collapsed"]);

    // Two grouped + four ungrouped properties.
    assert_eq!(script.properties.len(), 6);
    let title = &script.properties[0];
    assert_eq!(title.name, "Title");
    assert_eq!(title.kind, PscPropertyKind::Auto);
    assert_eq!(title.default.as_deref(), Some("\"Untitled\""));
    assert_eq!(title.group.as_deref(), Some("Display"));
    let count = &script.properties[1];
    assert_eq!(count.flags, vec!["Hidden"]);
    assert_eq!(count.group.as_deref(), Some("Display"));

    let speed = &script.properties[2];
    assert_eq!(speed.kind, PscPropertyKind::Auto);
    assert_eq!(speed.default.as_deref(), Some("1.0"));
    assert_eq!(speed.group, None);

    let greeting = &script.properties[3];
    assert_eq!(greeting.kind, PscPropertyKind::AutoReadOnly);
    assert_eq!(greeting.default.as_deref(), Some("\"Hello world!\""));

    let full = &script.properties[4];
    assert_eq!(full.kind, PscPropertyKind::Full);
    assert!(full.has_getter && full.has_setter);
    assert_eq!(
        full.docstring.as_deref(),
        Some("A full property with a getter and a setter.")
    );

    let read_only = &script.properties[5];
    assert_eq!(read_only.kind, PscPropertyKind::Full);
    assert!(read_only.has_getter && !read_only.has_setter);
}

#[test]
fn parses_functions_events_and_states() {
    let script = PscScript::parse(FULL_SCRIPT).unwrap();

    // Empty-state functions: DoNothing, AddTwo, WithDefaults,
    // NativeThing.
    assert_eq!(script.functions.len(), 4);
    let add = &script.functions[1];
    assert_eq!(add.name, "AddTwo");
    assert_eq!(add.return_type.as_deref(), Some("int"));
    assert!(add.is_global);
    assert!(!add.is_native);
    assert_eq!(add.params.len(), 2);
    assert_eq!(add.params[0].name, "a");
    assert_eq!(add.params[1].type_name, "int");

    let defaults = &script.functions[2];
    assert_eq!(defaults.params[0].default.as_deref(), Some("1"));
    // The comma inside the string default must not split params.
    assert_eq!(defaults.params.len(), 2);
    assert_eq!(defaults.params[1].default.as_deref(), Some("\"x,y\""));

    let native = &script.functions[3];
    assert!(native.is_native);
    assert_eq!(native.return_type, None);

    // Empty-state events: plain + remote.
    assert_eq!(script.events.len(), 2);
    assert_eq!(script.events[0].name, "OnActivate");
    assert_eq!(script.events[0].object_type, None);
    assert_eq!(script.events[0].params.len(), 1);
    assert_eq!(
        script.events[1].object_type.as_deref(),
        Some("ObjectReference")
    );
    assert_eq!(script.events[1].params.len(), 2);

    // States.
    assert_eq!(script.states.len(), 2);
    let my_state = script.state("MyState").unwrap();
    assert!(!my_state.auto);
    assert_eq!(my_state.functions.len(), 1);
    assert_eq!(my_state.functions[0].name, "AddTwo");
    assert_eq!(my_state.events.len(), 1);
    let starting = script.auto_state().unwrap();
    assert_eq!(starting.name, "Starting");
    assert!(starting.auto);
    assert_eq!(starting.functions.len(), 1);

    assert_eq!(script.all_functions().len(), 6);
    assert_eq!(script.all_events().len(), 3);
}

#[test]
fn handles_comments_and_line_continuations() {
    let source = r#"
ScriptName Commented
;/ A block comment
   spanning lines, with a fake declaration inside:
   int notReal = 1
/;
int realOne = 1 ; trailing comment
string longText = "part one " + \
  "part two"
; A line comment with a ;/ inside it is still one line
"#;
    let script = PscScript::parse(source).unwrap();
    assert_eq!(script.name, "Commented");
    assert_eq!(script.variables.len(), 2);
    assert_eq!(script.variables[0].name, "realOne");
    assert_eq!(script.variables[1].name, "longText");
    assert_eq!(
        script.variables[1].default.as_deref(),
        Some("\"part one \" + \"part two\"")
    );
}

#[test]
fn handles_semicolons_and_braces_inside_strings() {
    let source = r#"
ScriptName Tricky
string withSemi = "a;b"
string withBrace = "a{b}c"
Function UsesStrings()
  Debug.Trace("EndFunction")
  Debug.Trace("not a ; comment")
EndFunction
"#;
    let script = PscScript::parse(source).unwrap();
    assert_eq!(script.variables.len(), 2);
    assert_eq!(script.variables[0].default.as_deref(), Some("\"a;b\""));
    assert_eq!(script.variables[1].default.as_deref(), Some("\"a{b}c\""));
    assert_eq!(script.functions.len(), 1);
}

#[test]
fn keywords_are_case_insensitive() {
    let source = r#"
scriptname Lower
FUNCTION DoStuff()
endFunction
EVENT OnInit()
endevent
"#;
    let script = PscScript::parse(source).unwrap();
    assert_eq!(script.name, "Lower");
    assert_eq!(script.functions.len(), 1);
    assert_eq!(script.events.len(), 1);
}

#[test]
fn struct_blocks_are_skipped() {
    // Structs are Fallout 4 only; the parser tolerates and skips
    // them rather than failing on a ported script.
    let source = r#"
ScriptName WithStruct
Struct MyStruct
  int x
  float y
EndStruct
int after = 1
"#;
    let script = PscScript::parse(source).unwrap();
    assert_eq!(script.variables.len(), 1);
    assert_eq!(script.variables[0].name, "after");
}

#[test]
fn rejects_missing_script_name() {
    let source = "int x = 1\n";
    let err = PscScript::parse(source).unwrap_err();
    assert_eq!(err.line, 1);
    assert!(err.message.contains("ScriptName"));
    let err = PscScript::parse("").unwrap_err();
    assert!(err.message.contains("ScriptName"));
}

#[test]
fn rejects_unterminated_blocks() {
    let err = PscScript::parse("ScriptName A\nFunction Foo()\n").unwrap_err();
    assert!(err.message.contains("unterminated function"));
    let err = PscScript::parse("ScriptName A\nState S\n").unwrap_err();
    assert!(err.message.contains("unterminated state"));
    let err = PscScript::parse("ScriptName A\nGroup G\n").unwrap_err();
    assert!(err.message.contains("unterminated group"));
    let err = PscScript::parse("ScriptName A\nint Property P\n").unwrap_err();
    assert!(err.message.contains("unterminated property"));
}

#[test]
fn rejects_unrecognized_lines_and_bad_declarations() {
    let err = PscScript::parse("ScriptName A\nDoSomethingWeird\n").unwrap_err();
    assert!(err.message.contains("unrecognized line"));
    let err = PscScript::parse("ScriptName A\nint 123bad = 1\n").unwrap_err();
    assert!(err.message.contains("unrecognized line"));
    // AutoReadOnly without a value is malformed.
    let err = PscScript::parse("ScriptName A\nstring Property P AutoReadOnly\n").unwrap_err();
    assert!(err.message.contains("AutoReadOnly"));
    let err = PscScript::parse("ScriptName A\nEndFunction\n").unwrap_err();
    assert!(err.message.contains("without a matching block"));
    let err = PscScript::parse("ScriptName A\nScriptName B\n").unwrap_err();
    assert!(err.message.contains("first line"));
}

#[test]
fn rejects_unterminated_comments_and_strings() {
    let err = PscScript::parse("ScriptName A\n;/ never closed\nint x = 1\n").unwrap_err();
    assert!(err.message.contains("unterminated block comment"));
    let err = PscScript::parse("ScriptName A\n{never closed\n").unwrap_err();
    assert!(err.message.contains("unterminated documentation comment"));
    let err = PscScript::parse("ScriptName A\nstring s = \"oops\n").unwrap_err();
    assert!(err.message.contains("unterminated string"));
}

#[test]
fn rejects_wrong_contents_in_blocks() {
    let err = PscScript::parse("ScriptName A\nState S\nint x = 1\nEndState\n").unwrap_err();
    assert!(err.message.contains("only functions and events"));
    let err = PscScript::parse("ScriptName A\nGroup G\nFunction F()\nEndFunction\nEndGroup\n")
        .unwrap_err();
    assert!(err.message.contains("only properties"));
    let err = PscScript::parse(
        "ScriptName A\nint Property P\nFunction NotAGetter()\nEndFunction\nEndProperty\n",
    )
    .unwrap_err();
    assert!(err.message.contains("only Get and Set"));
}
