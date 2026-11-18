//! Papyrus script runtime for Dovahengine (M7).
//!
//! Executes compiled Papyrus bytecode (`.pex`, parsed by
//! `dovah-pex`) on script instances attached to game forms (via
//! `dovah-esm` VMAD fields). Follows
//! `docs/script-runtime-design.md`: runtime values with string
//! interning, a frame-based interpreter, an instance store with
//! VMAD initialization, state-aware function resolution, a native
//! registry (with latent natives), and a time-sliced scheduler
//! that runs script stacks on a per-tick instruction budget.
//!
//! Two things the public PEX documentation does not settle —
//! jump-target semantics and call operand order — are isolated in
//! the [`semantics`] module, proven against the real scripts in
//! M7.1 (jumps are relative to the jump instruction; calls are
//! method-first, destination-last). Nothing else in this crate
//! depends on those choices.
//!
//! This crate contains no Bethesda code and ships no game data: it
//! runs scripts from `.pex` files the user already owns.

pub mod error;
pub mod instance;
pub mod native;
pub mod resolve;
pub mod semantics;
pub mod value;
pub mod vm;

pub use error::RuntimeError;
pub use instance::{InstanceId, InstanceStore, ScriptInstance};
pub use native::{NativeDef, NativeFn, NativeOutcome, NativeRegistry};
pub use resolve::{resolve_function, resolve_parent_function, ScriptRegistry};
pub use semantics::Semantics;
pub use value::{CastContext, RuntimeValue, StringInterner};
pub use vm::{Frame, Stack, StackOutcome, Vm};
