//! How the two undocumented bytecode points are read.
//!
//! M7 guessed both from the UESP table's wording and got both
//! wrong. M7.1 settled them against the real scripts — all 13,494
//! `.pex` files from Dylan's Skyrim - Misc.bsa, 289,644
//! instructions — with the `pex_sweep` example and the
//! `pex_disasm` example. The readings below are proven, not
//! assumed; the evidence is cited in place so a future reader can
//! re-check it.
//!
//! # 1. Jump targets are RELATIVE to the jump instruction
//!
//! `target = pc + label`, where `pc` is the index of the jump
//! instruction itself. A target of exactly `instruction_count`
//! (one past the last instruction) means "fall off the end", an
//! implicit `return None`.
//!
//! Evidence (sweep over 36,362 jump instructions):
//! - 579 jumps have **negative** labels — impossible for absolute
//!   indices. 0 labels fall past the end.
//! - 36,362/36,362 jumps are valid under `pc + label`;
//!   35,783/36,362 under the absolute reading. Absolute is
//!   disproven; relative-to-jump is consistent with every jump.
//! - Relative to the *next* instruction (`pc + 1 + label`) is
//!   disproven by control flow. In
//!   `AtronachForgeSCRIPT.ScanForRecipes` (61 instructions, debug
//!   line numbers present):
//!   - `jmp -25` at pc 30 targets instruction 5, the first
//!     instruction of the loop condition (line 65). `pc + 1 +
//!     label` = 6 would skip the first comparison and reuse a
//!     stale temp.
//!   - `jmpf` at pc 7, label 3, targets 10, the second half of
//!     the `&&` condition. `pc + 1 + label` = 11 would enter the
//!     loop body with the condition false.
//!   - `jmpf` at pc 32, label 28, targets 60, the `return false`.
//!     `pc + 1 + label` = 61 is one past the end.
//!   - `jmp 2` at pc 59 targets 61 (one past the end — dead code
//!     after a `return`, jumping off the end). Under
//!     `pc + 1 + label` that is 62, out of range.
//!
//! # 2. Call operands are method-first, destination-last
//!
//! - `callmethod`: `(method, object, dest, args...)`
//! - `callparent`: `(method, dest, args...)`
//! - `callstatic`: `(script, method, dest, args...)`
//!
//! Evidence (sweep over 112,513 `callmethod`, 66 `callparent`,
//! 10,368 `callstatic` instructions):
//! - Under this reading, every `callstatic` resolves: 10,368 /
//!   10,368 calls hit a real declared function (113 distinct
//!   targets), and the call arg counts match the declared param
//!   counts 10,368 / 10,368. `callparent` resolves 66 / 66, arg
//!   counts match 66 / 66. `callmethod` shape conforms 112,513 /
//!   112,513 (dest is `None` or an identifier, method is an
//!   identifier).
//! - Under M7's dest-first reading the "method" position held
//!   compiler temp names (`::nonevar`, `::temp0`–`::temp27`) and
//!   the "script" position held function names (`wait`,
//!   `getplayer`, `trace`) — resolution 0%.
//! - Examples from `AtronachForgeSCRIPT.ScanForRecipes`:
//!   `callmethod getSize, Recipes, ::temp2` (method, object,
//!   dest); `callstatic utility, wait, ::nonevar, 0.33` (script,
//!   method, dest, arg). From
//!   `ArenaTransitionCombatQuest.SetupTransition`:
//!   `callparent Setup, ::nonevar` (method, dest).
//!
//! One oddity the sweep found: a single `jmpf` whose condition is
//! a literal `Bool(true)` instead of a variable
//! (`StatsPlayerPosition.Main`, pc 0 — the compiler's output for
//! a `while (true)` loop). The interpreter resolves jump
//! conditions through the normal argument path, so literals work;
//! nothing special is needed here.

use crate::error::RuntimeError;
use dovah_pex::Value;

/// The runtime's reading of the two bytecode points. The readings
/// are proven (see the module docs), so this is a unit struct —
/// there is nothing left to switch. It stays a struct (rather
/// than free functions) so the interpreter keeps one place to
/// consult, and so a future format variant has somewhere to hang
/// a switch if one is ever needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Semantics;

impl Semantics {
    /// Resolve a jump target to an absolute instruction index.
    /// `current_pc` is the index of the jump instruction itself.
    /// Errors when the target falls outside the function. A target
    /// of exactly `instruction_count` (one past the last
    /// instruction) is allowed: it means "fall off the end", which
    /// the interpreter treats as an implicit return.
    pub fn jump_target(
        &self,
        current_pc: usize,
        label: i32,
        instruction_count: usize,
    ) -> Result<usize, RuntimeError> {
        let target = current_pc as i64 + i64::from(label);
        if target < 0 || target as usize > instruction_count {
            return Err(RuntimeError::InvalidJumpTarget {
                label,
                count: instruction_count,
            });
        }
        Ok(target as usize)
    }

    /// Parse the fixed arguments of a `callmethod` instruction:
    /// `(method, object, dest, args...)`. The parser guarantees
    /// at least 3 fixed arguments.
    pub fn callmethod_operands<'a>(
        &self,
        args: &'a [Value],
    ) -> Result<MethodCall<'a>, RuntimeError> {
        let method = identifier(&args[0]).ok_or(RuntimeError::BadOperands {
            op: "callmethod",
            detail: "method name is not an identifier",
        })?;
        let dest = identifier_or_none(&args[2]).ok_or(RuntimeError::BadOperands {
            op: "callmethod",
            detail: "destination is not an identifier",
        })?;
        Ok(MethodCall {
            dest,
            object: &args[1],
            method,
            args: &args[3..],
        })
    }

    /// Parse the fixed arguments of a `callparent` instruction:
    /// `(method, dest, args...)`. The parser guarantees at least
    /// 2 fixed arguments.
    pub fn callparent_operands<'a>(
        &self,
        args: &'a [Value],
    ) -> Result<ParentCall<'a>, RuntimeError> {
        let method = identifier(&args[0]).ok_or(RuntimeError::BadOperands {
            op: "callparent",
            detail: "method name is not an identifier",
        })?;
        let dest = identifier_or_none(&args[1]).ok_or(RuntimeError::BadOperands {
            op: "callparent",
            detail: "destination is not an identifier",
        })?;
        Ok(ParentCall {
            dest,
            method,
            args: &args[2..],
        })
    }

    /// Parse the fixed arguments of a `callstatic` instruction:
    /// `(script, method, dest, args...)`. The parser guarantees at
    /// least 3 fixed arguments.
    pub fn callstatic_operands<'a>(
        &self,
        args: &'a [Value],
    ) -> Result<StaticCall<'a>, RuntimeError> {
        let script = identifier(&args[0]).ok_or(RuntimeError::BadOperands {
            op: "callstatic",
            detail: "script name is not an identifier",
        })?;
        let method = identifier(&args[1]).ok_or(RuntimeError::BadOperands {
            op: "callstatic",
            detail: "method name is not an identifier",
        })?;
        let dest = identifier_or_none(&args[2]).ok_or(RuntimeError::BadOperands {
            op: "callstatic",
            detail: "destination is not an identifier",
        })?;
        Ok(StaticCall {
            dest,
            script,
            method,
            args: &args[3..],
        })
    }
}

/// The parsed operands of a `callmethod` instruction.
#[derive(Debug, Clone, PartialEq)]
pub struct MethodCall<'a> {
    /// Where the return value goes (`None` when the call discards
    /// it — the destination is a `Value::None`).
    pub dest: Option<&'a str>,
    /// The object the method is called on.
    pub object: &'a Value,
    /// The method name.
    pub method: &'a str,
    /// The call arguments.
    pub args: &'a [Value],
}

/// The parsed operands of a `callparent` instruction.
#[derive(Debug, Clone, PartialEq)]
pub struct ParentCall<'a> {
    pub dest: Option<&'a str>,
    pub method: &'a str,
    pub args: &'a [Value],
}

/// The parsed operands of a `callstatic` instruction.
#[derive(Debug, Clone, PartialEq)]
pub struct StaticCall<'a> {
    pub dest: Option<&'a str>,
    /// The script the global function belongs to.
    pub script: &'a str,
    pub method: &'a str,
    pub args: &'a [Value],
}

/// An identifier argument, or `None` for a `Value::None` (a call
/// whose return value is discarded). Anything else is a malformed
/// instruction.
fn identifier_or_none(value: &Value) -> Option<Option<&str>> {
    match value {
        Value::None => Some(None),
        Value::Identifier(name) => Some(Some(name.as_str())),
        _ => Option::None,
    }
}

/// An identifier argument; anything else is a malformed
/// instruction.
fn identifier(value: &Value) -> Option<&str> {
    match value {
        Value::Identifier(name) => Some(name.as_str()),
        _ => Option::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(name: &str) -> Value {
        Value::Identifier(name.to_string())
    }

    #[test]
    fn jump_targets_are_relative_to_the_jump() {
        let sem = Semantics;
        // The ScanForRecipes loop-back jump: pc 30, label -25,
        // 61 instructions -> instruction 5, the loop condition.
        assert_eq!(sem.jump_target(30, -25, 61).unwrap(), 5);
        // Forward jump: pc 7, label 3 -> 10.
        assert_eq!(sem.jump_target(7, 3, 61).unwrap(), 10);
        // One past the end is allowed (fall off the end = return).
        assert_eq!(sem.jump_target(59, 2, 61).unwrap(), 61);
        // Out of range in either direction is an error.
        assert!(sem.jump_target(59, 3, 61).is_err());
        assert!(sem.jump_target(0, -1, 61).is_err());
    }

    #[test]
    fn call_operands_method_first_dest_last() {
        let sem = Semantics;
        // callmethod getSize, Recipes, ::temp2 (ScanForRecipes).
        let args = vec![id("getSize"), id("Recipes"), id("::temp2")];
        let call = sem.callmethod_operands(&args).unwrap();
        assert_eq!(call.method, "getSize");
        assert_eq!(call.object, &id("Recipes"));
        assert_eq!(call.dest, Some("::temp2"));
        assert!(call.args.is_empty());

        // callmethod with a discarded return and an argument.
        let args = vec![id("Open"), id("Self"), Value::None, Value::Int(1)];
        let call = sem.callmethod_operands(&args).unwrap();
        assert_eq!(call.method, "Open");
        assert_eq!(call.dest, Option::None);
        assert_eq!(call.args, &[Value::Int(1)]);

        // callparent Setup, ::nonevar (ArenaTransitionCombatQuest).
        let parent_args = [id("Setup"), id("::nonevar")];
        let parent = sem.callparent_operands(&parent_args).unwrap();
        assert_eq!(parent.method, "Setup");
        assert_eq!(parent.dest, Some("::nonevar"));
        assert!(parent.args.is_empty());

        // callstatic utility, wait, ::nonevar, 0.33 (ScanForRecipes).
        let static_args = [id("utility"), id("wait"), Value::None, Value::Float(0.33)];
        let static_call = sem.callstatic_operands(&static_args).unwrap();
        assert_eq!(static_call.script, "utility");
        assert_eq!(static_call.method, "wait");
        assert_eq!(static_call.dest, Option::None);
        assert_eq!(static_call.args, &[Value::Float(0.33)]);

        // A non-identifier method name is malformed.
        assert!(sem
            .callmethod_operands(&[Value::Int(3), id("Self"), id("r")])
            .is_err());
        // A non-identifier destination is malformed.
        assert!(sem
            .callstatic_operands(&[id("Utility"), id("Wait"), Value::Int(3)])
            .is_err());
    }
}
