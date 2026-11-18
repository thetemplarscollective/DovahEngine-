# Papyrus runtime — design (Phase 3, milestone M7)

This is a design, not an implementation. M5 built the parser
(`dovah-pex`); this document says what M7 must build on top of it
to actually *run* Papyrus scripts, and how the pieces fit together.
Nothing here is code yet.

## What the parser hands the runtime

`dovah-pex` produces, per `.pex` file:

- `PexFile` — the script's objects (normally one), string table,
  debug info, user flags.
- `PexObject` — parent script name, auto state, `variables` (name,
  type, default `Value`), `properties` (kind, flags, auto var or
  get/set handler `Function`s), `states` (each a list of named
  functions).
- `Function` — return type, params, locals, and `instructions`:
  `Instruction { op, args: Vec<Value> }`, where each `Value` is
  `None`, `Identifier(name)`, `Str(literal)`, `Int`, `Float`, or
  `Bool`.

The runtime never re-parses bytecode. It loads `PexFile`s once
(from the BSAs, via `dovah-bsa`), indexes them by script name, and
executes from the parsed form.

## 1. Value model

Papyrus values are dynamically typed at runtime, even though the
source is statically typed. The runtime needs its own value type,
richer than the parser's `Value` (which only covers literals and
names):

```rust
enum RuntimeValue {
    None,
    Int(i32),
    Float(f32),
    Bool(bool),
    Str(String),              // interned in practice; see below
    Object(InstanceId),       // a script instance on a form
    Array(Rc<RefCell<Vec<RuntimeValue>>>),
}
```

Notes:

- **Strings are interned.** Papyrus strings are case-insensitive
  in the engine's string pool, and scripts compare them constantly.
  The runtime should intern strings (one canonical `Rc<str>` per
  distinct lowercase form) so equality is a pointer comparison and
  memory stays bounded. Docstrings and debug info do not go through
  the pool.
- **Objects are references, not values.** An `Object` value points
  at a script instance (see §3). Two variables holding the same
  object see each other's changes.
- **Arrays are references too**, and hold one element type
  (enforced at creation, from the `array_create` operand and the
  variable's declared type).
- **`Var` is just "any `RuntimeValue`"** — the parser's `Value`
  has no Var variant because a literal is always a concrete type;
  a variable *declared* `var` holds whatever it was last assigned.
- **Casting** (`cast` opcode, `as` in source) follows Papyrus
  rules: int↔float converts numerically, anything→string formats,
  object→object casts check the script hierarchy (an `Actor` is an
  `ObjectReference`; a failed downcast yields `None`). Cast
  semantics are a small table M7 must pin down and test.

## 2. The execution model: frames, not an operand stack

Papyrus bytecode is three-address code: arithmetic writes to a
named destination (`iadd dest, lhs, rhs`), `assign` stores into a
named variable, and calls carry their arguments in the instruction
itself. There is no general operand stack for expressions — the
compiler turns temporaries into locals. So the interpreter is a
simple frame machine:

```rust
struct Frame {
    function: Rc<Function>,   // parsed bytecode + signature
    instance: InstanceId,     // `Self`
    pc: usize,                // instruction pointer
    locals: HashMap<String, RuntimeValue>,  // params + locals
    dest: Option<Dest>,       // where the return value goes
}
```

A running script is a **stack** of frames (Papyrus's own term for
a call chain). The interpreter loop is:

1. Fetch `frame.function.instructions[frame.pc]`.
2. Resolve each argument: a `Value::Identifier(name)` reads the
   named variable (locals first, then the instance's script
   variables, then `Self`/`Parent`); any other `Value` is a
   literal.
3. Execute the opcode: arithmetic/comparison writes the result
   into the destination variable; `jmp`/`jmpt`/`jmpf` set `pc`;
   `return` pops the frame and delivers its value to the caller's
   destination.
4. `callmethod` / `callparent` / `callstatic` push a new frame
   (or invoke a native — see §5); `propget` / `propset` go through
   the property machinery (§4).

Two questions were open when M7 wrote the loop, and both were
settled in M7.1 by disassembly against all 13,494 real `.pex`
files (M7 guessed both wrong from the UESP wording — the guesses
live on only as a warning in `semantics.rs`):

- Jump targets are **relative to the jump instruction**: `target
  = pc + label`. A target equal to the instruction count falls
  off the end (implicit return).
- Call operands are **method-first, destination-last**:
  `callmethod (method, object, dest, args...)`,
  `callparent (method, dest, args...)`,
  `callstatic (script, method, dest, args...)`. The same
  method-first pattern holds for `propget`, `propset`, and
  `array_findelement`; arithmetic and friends stay dest-first.

The evidence is cited in `crates/dovah-papyrus/src/semantics.rs`
and `docs/formats/pex.md`.

The interpreter must also enforce a per-frame execution budget
(see §6): a script stuck in a loop must not freeze the game.

## 3. Object and instance model

A **script** is the compiled definition (`PexObject`). An
**instance** is that script attached to one game object, with its
own variable values, property values, and current state:

```rust
struct ScriptInstance {
    script: String,                       // script name
    form: FormId,                         // the object it is attached to
    variables: HashMap<String, RuntimeValue>,
    state: String,                        // "" = empty state
}
```

- Instances are created when a form with scripts loads: the ESM
  `VMAD` field on the record lists the attached scripts and the
  **initial property values** for each (set in the Creation Kit).
  Instance creation runs: allocate variables at their PEX
  defaults, apply the VMAD property values, set the state to the
  script's auto state, then queue the `OnInit` event.
- Auto properties are backed by hidden variables (the PEX
  `auto_var` name); reading/writing them is a variable access.
  Full properties run their get/set handler functions.
- The instance store is keyed by (form ID, script name). Instances
  persist for the life of the save; their variables and state are
  part of save data (M7's save/load work).
- Inheritance: an instance's script extends a parent script
  (`PexObject.parent`). Variables and functions resolve up the
  chain; `Parent.Function()` calls the parent's version
  (`callparent`).

## 4. Function resolution and states

Which function runs for a call depends on the instance's current
state. Resolution order for a call to `F` on an instance of script
`S` in state `T`:

1. `S`'s state `T` version of `F`.
2. The parent script's state `T` version of `F`.
3. `S`'s empty-state version of `F`.
4. The parent script's empty-state version of `F`.

`GotoState("T")` changes the instance's state (and fires
`OnBeginState`/`OnEndState`); `GotoState("")` returns to the empty
state. `GetState()` returns the current state name. The resolver
is a pure function of (script, state, name) over the loaded
`PexFile`s, so it is easy to unit-test with synthetic scripts.

## 5. Native functions

Functions flagged `native` in the PEX carry no bytecode — the
engine implements them. The runtime needs a native registry:

```rust
// Keyed by (script name, function name), e.g. ("Debug", "Trace").
type NativeFn = fn(&mut Vm, args: Vec<RuntimeValue>) -> RuntimeValue;
```

- Registration is explicit and typed at the boundary: each native
  declares its parameter types and return type, and the VM checks
  argument counts/types before calling into Rust. A call to a
  native the engine has not implemented is a runtime error that
  names the missing function — never a silent no-op.
- The native surface is large (Debug, Utility, Game, Math,
  StringUtil, and every script type's methods: Actor,
  ObjectReference, Quest, ...). M7 starts with a minimal set that
  lets test scripts run — `Debug.Trace`, `Utility.Wait`,
  `Utility.GetRandomInt`, `Game.GetPlayer`, `Math` helpers — and
  grows it alongside gameplay, driven by what real scripts call.
- **Latent natives** (`Utility.Wait`, `WaitMenuMode`) do not
  return immediately: they suspend the calling stack and resume it
  later (see §6). The registry must mark which natives are latent.

## 6. Scheduling: stacks, events, and latent functions

Papyrus does not run scripts to completion inline. Work arrives as
events, and the VM processes a queue of stacks with a time budget:

- **Event queue.** The engine posts events — `OnInit` when a
  script attaches, `OnActivate` when the player activates an
  object, `OnUpdate` from `RegisterForUpdate` timers, quest stage
  events, custom events (`SendCustomEvent`), and remote events.
  Each queued event becomes a new stack (a frame for the event
  function on the target instance).
- **Time-sliced execution.** Each game tick, the VM runs queued
  stacks until a small instruction or time budget is spent, then
  yields. A stack that is still running is resumed next tick from
  its saved `pc` and frames. This is what keeps a heavy script
  from stalling the renderer, and it matters more on a phone than
  on a PC.
- **Latent functions.** When a running stack calls a latent native
  (e.g. `Wait(2.0)`), the stack is parked with a wake time and
  removed from the runnable queue until then. `RegisterForUpdate`
  is the same mechanism with a repeating timer.
- **Ordering.** Papyrus in Skyrim runs scripts on their own
  thread; our VM runs on the Bevy schedule (a dedicated system,
  single-threaded to start). Cross-thread script execution is
  explicitly out of scope for M7 — determinism and debuggability
  come first.

## 7. What M7 builds, in order

1. **Runtime values + casts.** `RuntimeValue`, string interning,
   conversion table, equality. Unit-tested without any scripts.
2. **Interpreter core.** Frames, the fetch-execute loop, and the
   non-call opcodes (arithmetic, compare, assign, cast, jumps,
   return, arrays, string concat). Verified against synthetic PEX
   files built by hand — e.g. a function that adds two numbers
   and returns, a loop that counts to ten.
3. **Calls and frames.** `callmethod`/`callparent`/`callstatic`,
   native registry, the first natives (`Debug.Trace`,
   `Utility.GetRandomInt`, `Math`). A synthetic script that calls
   a script function and a native, end to end.
4. **Instances and properties.** Instance store, VMAD initial
   values (needs the ESM `VMAD` field parsed — a dovah-esm
   follow-up), auto-property backing variables, full-property
   get/set handlers, `propget`/`propset`.
5. **States.** The resolver from §4, `GotoState`/`GetState`, and
   `OnBeginState`/`OnEndState` events.
6. **Events and scheduling.** The event queue, the time-sliced
   scheduler, `OnInit`/`OnActivate` dispatch from the engine,
   latent functions (`Wait`), update timers.
7. **Save/load.** Serialize instances (variables, state) and
   runnable/parked stacks into the save format, and restore them.
   This lands with M7's save system, not before.
8. **Real-script proving.** Run pure functions from real game
   scripts (disassemble with a small tool built on `dovah-pex`,
   execute, compare against expected behavior), then graduate to
   in-game scripts: a door that opens on activation, a quest
   stage advance. DONE in M7.1
   (`crates/dovah-papyrus/examples/prove_real_scripts.rs`,
   pinned in `crates/dovah-papyrus/tests/real_scripts.rs`):
   `CWScript.HoldName` 11/11 and `CraftSmithing.GetMinSkill` 2/2
   correct; all 5 Sleeping Giant Inn scripts' `OnInit` clean;
   OnInit census over all 13,494 scripts — 106 clean, 77 errored
   (59 on 20 missing game-system natives, 18 on VMAD data the
   census does not provide), 0 parked, 0 stuck, 0 type errors.
   Graduation: the inn's room trigger runs end to end
   (`OnLoad` → `OnTriggerEnter` → `playerInRoom = true`) and
   stops at the single unimplemented native
   `Quest.SetCurrentStageID`, behind the script wrapper
   `Quest.SetStage`.

## 8. Deliberate non-goals for M7

- **No Papyrus compiler.** The runtime executes `.pex` only. The
  PSC parser exists to understand mod sources and to help debug;
  compiling `.psc` to `.pex` is a separate project, if it ever
  happens.
- **No JIT.** A tree-walking interpreter over the parsed
  instructions is fast enough for Papyrus workloads to start
  with. If profiling shows the interpreter is hot, the first
  optimization is denser bytecode (pre-resolved variable slots
  instead of name lookups), not a JIT.
- **No SKSE.** SKSE scripts call into SKSE's native extensions,
  which do not exist here. Scripts that depend on SKSE natives
  fail loudly at the native registry, naming the missing
  function.
