# PEX / PSC — Papyrus scripts

Source: UESP wiki, "Skyrim Mod:Compiled Script File Format"
(`https://en.uesp.net/wiki/Skyrim_Mod:Compiled_Script_File_Format`)
and "Skyrim Mod:File Format Conventions" for the primitive types;
PSC grammar from the papyrus-lang wiki ("Papyrus Scripts" and
"Papyrus Types") and the Creation Kit wiki. Notes below are our own
summary of those public specs.

## What they are

Papyrus is Skyrim's scripting language — the game logic behind
quests, dialogue, traps, AI packages, and everything that moves or
reacts. Two forms:

- `.pex` — the compiled bytecode the game actually executes. Every
  script the game runs exists as a `.pex` in the data files
  (`Data/Scripts/*.pex`, inside the BSA archives).
- `.psc` — human-readable Papyrus source, as written in the
  Creation Kit. The game never runs these; they exist for modders
  and for the compiler. The vanilla sources ship in
  `Data/Scripts/Source/`.

**PEX is big-endian.** Every multi-byte integer and float in a
`.pex` file is stored most-significant byte first, regardless of
platform — the opposite of the ESM and BSA formats, which are
little-endian. This is the easiest mistake to make when writing the
reader.

Skyrim supports PEX versions 3.0 to 3.1 (major 3). Special Edition
scripts are 3.1; UESP notes Dawnguard, Hearthfire, and Dragonborn
scripts are 3.2. The reader accepts major 3, minor 0–2, and records
the version it found.

## PEX layout

```
Header (magic, version, game ID, compilation time, source/user/machine names)
String table
Debug info (optional)
User flags
Objects (usually exactly one — the script itself)
```

### Header

| Field | Type | Notes |
| --- | --- | --- |
| magic | u32 | `0xFA57C0DE` ("FASTCODE?") |
| majorVersion | u8 | 3 |
| minorVersion | u8 | 1 for SE; DLC scripts are 2 |
| gameId | u16 | 1 = Skyrim |
| compilationTime | u64 | Unix timestamp of compilation |
| sourceFileName | wstring | The `.psc` file this was compiled from |
| username | wstring | Username used to compile |
| machinename | wstring | Machine name used to compile |
| stringTable | String Table | Below |
| debugInfo | Debug Info | Below |
| userFlagCount | u16 | |
| userFlags | User Flag[] | Below |
| objectCount | u16 | |
| objects | Object[] | Below |

`wstring` (per the UESP conventions): a u16 big-endian byte length
followed by that many bytes, no terminator, Windows-1252 encoded.

### String table

| Field | Type | Notes |
| --- | --- | --- |
| count | u16 | |
| strings | wstring[count] | |

Every name, type, and string literal in the file is an index into
this table — there is no separate "type table": a type is just a
string-table entry (e.g. `Int`, `Actor`, `MyScript`). Index 0 is
the empty string by convention, and the empty string is how the
default (empty) state and "no docstring" are represented. All
indices are 0-based; the reader rejects an out-of-range index.

### Debug info

| Field | Type | Notes |
| --- | --- | --- |
| hasDebugInfo | u8 | If 0, no debug info is present and the rest is skipped |
| modificationTime | u64 | Unix timestamp of the source file's last modification |
| functionCount | u16 | |
| functions | Debug Function[] | |

Debug Function:

| Field | Type | Notes |
| --- | --- | --- |
| objectNameIndex | u16 | String-table index |
| stateNameIndex | u16 | String-table index |
| functionNameIndex | u16 | String-table index |
| functionType | u8 | Valid values 0–3; the mapping is not documented, so the raw value is kept |
| instructionCount | u16 | |
| lineNumbers | u16[] | One source line per instruction, in order |

### User flag

| Field | Type | Notes |
| --- | --- | --- |
| nameIndex | u16 | String-table index (e.g. `Hidden`, `Conditional`) |
| flagIndex | u8 | Bit index in the `user_flags` bitmask |

User flags are the named bits that objects, variables, properties,
and functions reference through their u32 `user_flags` field.

### Object

| Field | Type | Notes |
| --- | --- | --- |
| nameIndex | u16 | String-table index |
| size | u32 | Size of the object data **including these 4 size bytes** |
| data | Object Data[size − 4] | |

The size includes itself, so the object data is `size − 4` bytes.
The reader parses it from a bounded sub-reader and rejects trailing
bytes inside it, so one corrupt object cannot run into the next.

Object Data:

| Field | Type | Notes |
| --- | --- | --- |
| parentClassName | u16 | String-table index (the script this extends, e.g. `ObjectReference`) |
| docstring | u16 | String-table index |
| userFlags | u32 | Bitmask of user flags |
| autoStateName | u16 | String-table index; `""` if the script starts in the empty state |
| numVariables | u16 | |
| variables | Variable[] | |
| numProperties | u16 | |
| properties | Property[] | |
| numStates | u16 | |
| states | State[] | |

#### Variable

| Field | Type | Notes |
| --- | --- | --- |
| name | u16 | String-table index |
| typeName | u16 | String-table index |
| userFlags | u32 | |
| data | Variable Data | Default value |

#### Variable Data

The uniform value encoding, used for variable defaults and for
every instruction argument:

| Field | Type | Notes |
| --- | --- | --- |
| type | u8 | 0 = null, 1 = identifier, 2 = string, 3 = integer, 4 = float, 5 = bool |
| data | u16 | String-table index; identifier and string types only |
| data | i32 | Integer types only |
| data | f32 | Float types only |
| data | u8 | Bool types only (nonzero = true) |

Null has no payload. Identifier and string both carry a string-table
index; the difference is semantic — an identifier names a variable,
function, or property, while a string is a literal value.

#### Property

| Field | Type | Notes |
| --- | --- | --- |
| name | u16 | String-table index |
| type | u16 | String-table index |
| docstring | u16 | String-table index |
| userFlags | u32 | |
| flags | u8 | Bitfield: 1 = read, 2 = write, 4 = autovar |
| autoVarName | u16 | String-table index; present only if `flags & 4 != 0` |
| readHandler | Function | Present only if `(flags & 5) == 1` |
| writeHandler | Function | Present only if `(flags & 6) == 2` |

The handler rules mean: an auto property (`flags & 4`) stores its
hidden backing variable's name and no handlers — the compiler
generated trivial ones. A full property stores a get handler when
it is readable and not auto, and a set handler when it is writable
and not auto. So `0x1` = get-only, `0x2` = set-only, `0x3` = full
get+set, `0x5` = auto read-only, `0x7` = auto read+write.

#### State

| Field | Type | Notes |
| --- | --- | --- |
| name | u16 | String-table index; empty string for the default state |
| numFunctions | u16 | |
| functions | Named Function[] | |

Named Function:

| Field | Type | Notes |
| --- | --- | --- |
| functionName | u16 | String-table index |
| function | Function | |

#### Function

A function is nameless on its own — its name lives in the Named
Function that wraps it, or in the property that owns it as a
handler.

| Field | Type | Notes |
| --- | --- | --- |
| returnType | u16 | String-table index (`None` for void) |
| docstring | u16 | String-table index |
| userFlags | u32 | |
| flags | u8 | Bit 0 = global, bit 1 = native (no bytecode) |
| numParams | u16 | |
| params | Variable Type[] | |
| numLocals | u16 | |
| locals | Variable Type[] | |
| numInstructions | u16 | |
| instructions | Instruction[] | |

Variable Type:

| Field | Type | Notes |
| --- | --- | --- |
| name | u16 | String-table index |
| type | u16 | String-table index |

### Instruction

| Field | Type | Notes |
| --- | --- | --- |
| op | u8 | See the opcode table |
| arguments | Variable Data[] | Count depends on the opcode; the three call opcodes take varargs |

#### Opcodes

The "args" column gives the argument pattern. Each letter is one
Variable Data; `*` is a variable-length tail: a Variable Data
integer holding the count, followed by that many Variable Data
arguments.

| Op | Name | Args | Meaning |
| --- | --- | --- | --- |
| 00 | nop | — | Do nothing |
| 01 | iadd | SII | Integer add |
| 02 | fadd | SFF | Float add |
| 03 | isub | SII | Integer subtract |
| 04 | fsub | SFF | Float subtract |
| 05 | imul | SII | Integer multiply |
| 06 | fmul | SFF | Float multiply |
| 07 | idiv | SII | Integer divide |
| 08 | fdiv | SFF | Float divide |
| 09 | imod | SII | Integer remainder |
| 0A | not | SA | Negate truthiness — any value type, not just bool (see below) |
| 0B | ineg | SI | Negate an integer |
| 0C | fneg | SF | Negate a float |
| 0D | assign | SA | Store a variable |
| 0E | cast | SA | Type conversion |
| 0F | cmp_eq | SAA | a == b |
| 10 | cmp_lt | SAA | a < b |
| 11 | cmp_le | SAA | a <= b |
| 12 | cmp_gt | SAA | a > b |
| 13 | cmp_ge | SAA | a >= b |
| 14 | jmp | L | Unconditional branch |
| 15 | jmpt | AL | Branch if true |
| 16 | jmpf | AL | Branch if false |
| 17 | callmethod | NSS* | Call a method on an object |
| 18 | callparent | NS* | Call the parent's version of a function |
| 19 | callstatic | NNS* | Call a static/global function |
| 1A | return | A | Return from the function |
| 1B | strcat | SQQ | Concatenate two strings |
| 1C | propget | NSS | Read an instance property |
| 1D | propset | NSA | Write an instance property |
| 1E | array_create | Su | Create an array of a size |
| 1F | array_length | SS | Get an array's length |
| 20 | array_getelement | SSI | Get an array element |
| 21 | array_setelement | SIA | Set an array element |
| 22 | array_findelement | SSII | Find an element (4th arg = start index, default 0) |
| 23 | array_rfindelement | SSII | Find an element from the end (start index default −1) |

Argument letters: A = any value, I = integer, F = float, B =
boolean, S = string/identifier, N = identifier, u = unsigned
integer, Q = string (why Q is used is unclear), L = jump label (an
integer Variable Data). The letters describe the *typical* type of
each argument, but the wire format is uniform — every argument is a
Variable Data — so the parser uses the letters only to know how
many arguments to read, and does not enforce the types.

**Proven semantics (M7.1).** The two questions UESP leaves open
were settled by disassembly against all 13,494 real `.pex` files
from Dylan's Misc.bsa (see `crates/dovah-papyrus/src/semantics.rs`
for the evidence):

- **Jump targets are relative to the jump instruction itself:**
  `target = pc + label`, where `pc` is the jump instruction's own
  index. A target equal to the instruction count means "fall off
  the end" (implicit return). Proven by 579 negative labels
  (impossible under absolute indexing) and by control flow in
  `AtronachForgeSCRIPT.ScanForRecipes`, which is correct only
  under relative-to-the-jump (relative-to-the-next-instruction
  puts a jump one past the end).
- **Call operands are method-first, destination-last:**
  `callmethod (method, object, dest, args...)`,
  `callparent (method, dest, args...)`,
  `callstatic (script, method, dest, args...)`. Proven by
  10,368/10,368 static calls resolving to real script functions
  with matching argument counts — under a dest-first reading the
  "method" position held compiler temporaries and nothing
  resolved. The same method-first pattern holds for
  `propget (property, object, dest)`,
  `propset (property, object, value)`, and
  `array_findelement (array, dest, needle, start)`.
  Arithmetic, comparison, `assign`, `cast`, `strcat`,
  `array_create`, `array_length`, and `array_getelement` stay
  dest-first, as the table shows.
- **`not` applies Papyrus truthiness to any value**, not just
  bools: `None`, `0`, `0.0`, and `""` are false; any object or
  array reference is true. Real scripts `not` object references
  (`if (!myRef)`) and ints (`if (!myStage)`).
- **A variable whose PEX default is `None` starts at its type's
  zero value** (0, 0.0, false, `""`; objects, arrays, and `Var`
  stay `None`). Proven by `defaultOnEnter.OnInit`, which adds 1
  to a counter whose PEX default is `None` — that only works if
  the counter starts at 0.

## PSC — source syntax

A `.psc` file is the header line followed by declarations in any
order. Papyrus keywords are case-insensitive; names keep their
casing. Statements end at the line, unless the line ends in `\`,
which continues it onto the next line.

Comments, three kinds:

- `;` — line comment to the end of the line.
- `;/ ... /;` — block comment, may span lines.
- `{...}` — documentation comment, attached to the declaration it
  *follows* (script header, property, group, or function). It may
  span lines and shows up as a tooltip in the Creation Kit.

Strings are double-quoted, may not span lines, and use `\"`, `\\`,
`\n`, `\t` escapes. A `;` or `{` inside a string is not a comment.

### Header

```
ScriptName <name> [Extends <parent>] [Native] [flags...]
```

Script flags: `Conditional`, `Const`, `DebugOnly`, `BetaOnly`,
`Hidden`, `Native`, `Default`. `Native` scripts may not contain
data or states; `Const` scripts may not contain non-const
variables, auto properties, or states.

### Declarations

| Form | Syntax |
| --- | --- |
| Import | `Import <ScriptName>` |
| Variable | `<type> <name> [= <value>] [Conditional] [Const]` |
| Auto property | `<type> Property <name> [= <value>] Auto [flags]` |
| Auto read-only | `<type> Property <name> = <value> AutoReadOnly [flags]` |
| Full property | `<type> Property <name> [flags]`, then `Get`/`Set` functions, then `EndProperty` |
| Property group | `Group <name> [flags]` ... properties ... `EndGroup` |
| State | `[Auto] State <name>` ... functions/events ... `EndState` |
| Function | `[<type>] Function <name>(<params>) [Global] [Native] [flags]` ... `EndFunction` |
| Event | `Event <name>(<params>) [Native] [flags]` ... `EndEvent` |
| Remote event | `Event <ObjectType>.<name>(<sender>, <params>)` ... `EndEvent` |
| Custom event | `CustomEvent <name>` (declares an event the script can send) |

Property flags: `Conditional`, `Const`, `Hidden`, `Mandatory`.
Group flags: `Collapsed`, `CollapsedOnRef`, `CollapsedOnBase`.
Function/event flags: `DebugOnly`, `BetaOnly`.

Types are `Int`, `Float`, `Bool`, `String`, `Var`, any script name
as an object type, or any of those with an `[]` array suffix.
`Var` holds any value. Parameters may have defaults
(`int howMuch = 1`); a defaulted parameter forces defaults on every
parameter after it.

States: functions and events defined outside any state are in the
empty state — the default implementation. A state holds overrides
with identical names, return types, and parameter lists. `Auto
State` is the state the script starts in; only one state may be
auto. Resolution order when calling: current state's version, then
the parent's state version, then the empty state.

Structs (`Struct ... EndStruct`) are a Fallout 4 addition — Skyrim
Papyrus has none, and the PEX format has no struct section. The PSC
parser tolerates and skips struct blocks rather than failing on a
ported script, but does not parse them.

## Reader notes (dovah-pex)

- The PEX parser (`PexFile`) resolves all string-table indices at
  parse time into owned strings, and validates every index.
  Bytecode is parsed into instructions (opcode + `Value`
  arguments), including the varargs tail of the call opcodes, but
  it is **not executed** — execution is the Phase 3 runtime (see
  `docs/script-runtime-design.md`).
- Object data is parsed from a bounded sub-reader keyed off the
  size field, and trailing bytes — inside an object or after the
  last object — are rejected.
- Unknown opcodes are rejected: the parser cannot know how many
  arguments an unknown opcode takes, so it cannot resync.
- The PSC parser (`psc::PscScript`) is structural only: it reads
  headers, variables, properties, groups, states, and
  function/event signatures, and skips bodies. It is not a
  compiler and does not evaluate expressions; default values are
  kept as raw source text.
- The PSC parser is strict about structure (a missing `ScriptName`,
  an unterminated block, a line it cannot classify, or an
  unterminated comment/string is an error with a line number), but
  lenient about content it does not need (unknown doc comments are
  ignored, struct blocks are skipped).
- Real-data verification is DONE (M7.1): all 13,494 `.pex` files
  extracted from Dylan's Skyrim - Misc.bsa parse cleanly —
  13,494 objects, 52,812 functions (674 native), 289,644
  instructions. Versions in the real set: 9,930 files are 3.1
  and 3,564 are 3.2. One file, `tif__000c4428.pex`, failed to
  extract on a disk-quota error and is missing from the set. The
  jump-target semantics and call operand order were confirmed
  against this disassembly (see above); the sweep lives in
  `crates/dovah-pex/examples/pex_sweep.rs` and the disassembler
  in `crates/dovah-pex/examples/pex_disasm.rs`.
