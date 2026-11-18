//! Runtime errors: what can go wrong while a script runs. A script
//! that hits one of these stops (its stack is dropped and the error
//! is recorded on the VM), but the VM keeps running the other
//! stacks — one broken script must not kill the game. Every error
//! names what went wrong and where, so a missing native or a bad
//! call is diagnosable instead of a silent no-op.

use crate::instance::InstanceId;
use std::fmt;

/// An error raised while executing a Papyrus script.
#[derive(Debug, Clone, PartialEq)]
pub enum RuntimeError {
    /// A variable was read that is neither a local nor a script
    /// variable of the instance.
    UndefinedVariable { name: String },
    /// An opcode got a value of the wrong type.
    TypeMismatch { expected: String, found: String },
    /// Integer division or remainder by zero.
    DivisionByZero,
    /// A method, property, or function was called on `None`.
    CallOnNone { what: String },
    /// A script function was called that does not exist on the
    /// instance's script (or its parents).
    MissingFunction {
        script: String,
        state: String,
        function: String,
    },
    /// A script definition was needed (to create an instance, or
    /// for a static call) and is not loaded.
    MissingScript { script: String },
    /// A native function was called that the engine has not
    /// implemented. Names the function — never a silent no-op.
    MissingNative { script: String, function: String },
    /// A native was called with the wrong number of arguments.
    WrongArgCount {
        script: String,
        function: String,
        expected: usize,
        got: usize,
    },
    /// A jump target outside the function's instruction list.
    InvalidJumpTarget { label: i32, count: usize },
    /// A call or branch instruction whose operands are malformed
    /// (e.g. a method name that is not an identifier).
    BadOperands {
        op: &'static str,
        detail: &'static str,
    },
    /// A property was read or written that the script does not
    /// define.
    MissingProperty { script: String, property: String },
    /// A property was read that is write-only, or written that is
    /// read-only.
    PropertyAccess { script: String, property: String },
    /// The `cast` opcode could not determine the destination
    /// variable's declared type.
    CastWithoutType { dest: String },
    /// A cast that cannot be performed at all.
    BadCast { from: String, to: String },
    /// An array operation on a value that is not an array.
    NotAnArray,
    /// `array_create` on a destination whose declared type is not
    /// an array type.
    NotAnArrayType { dest: String, type_name: String },
    /// A native raised an error of its own.
    Native {
        script: String,
        function: String,
        message: String,
    },
    /// An instance ID that does not exist in the store.
    UnknownInstance { id: InstanceId },
    /// A VMAD property value could not be applied (e.g. its type
    /// does not match the declared property type).
    BadVmadValue { script: String, property: String },
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RuntimeError::UndefinedVariable { name } => {
                write!(f, "undefined variable '{name}'")
            }
            RuntimeError::TypeMismatch { expected, found } => {
                write!(f, "expected {expected}, found {found}")
            }
            RuntimeError::DivisionByZero => write!(f, "integer division by zero"),
            RuntimeError::CallOnNone { what } => write!(f, "called {what} on None"),
            RuntimeError::MissingFunction {
                script,
                state,
                function,
            } => {
                if state.is_empty() {
                    write!(f, "no function '{function}' on script '{script}'")
                } else {
                    write!(
                        f,
                        "no function '{function}' on script '{script}' in state '{state}'"
                    )
                }
            }
            RuntimeError::MissingScript { script } => {
                write!(f, "script '{script}' is not loaded")
            }
            RuntimeError::MissingNative { script, function } => {
                write!(
                    f,
                    "native function {script}.{function} is not implemented by the engine"
                )
            }
            RuntimeError::WrongArgCount {
                script,
                function,
                expected,
                got,
            } => {
                write!(
                    f,
                    "{script}.{function} takes {expected} argument(s), got {got}"
                )
            }
            RuntimeError::InvalidJumpTarget { label, count } => {
                write!(f, "jump target {label} outside 0..{count}")
            }
            RuntimeError::BadOperands { op, detail } => {
                write!(f, "malformed {op} instruction: {detail}")
            }
            RuntimeError::MissingProperty { script, property } => {
                write!(f, "no property '{property}' on script '{script}'")
            }
            RuntimeError::PropertyAccess { script, property } => {
                write!(
                    f,
                    "property '{property}' on script '{script}' is not accessible here"
                )
            }
            RuntimeError::CastWithoutType { dest } => {
                write!(f, "cannot determine the type of '{dest}' for a cast")
            }
            RuntimeError::BadCast { from, to } => write!(f, "cannot cast {from} to {to}"),
            RuntimeError::NotAnArray => write!(f, "array operation on a non-array value"),
            RuntimeError::NotAnArrayType { dest, type_name } => {
                write!(f, "'{dest}' is declared {type_name}, not an array type")
            }
            RuntimeError::Native {
                script,
                function,
                message,
            } => {
                write!(f, "native {script}.{function} failed: {message}")
            }
            RuntimeError::UnknownInstance { id } => {
                write!(f, "unknown script instance {}", id.0)
            }
            RuntimeError::BadVmadValue { script, property } => {
                write!(
                    f,
                    "VMAD value for {script}.{property} does not fit its type"
                )
            }
        }
    }
}

impl std::error::Error for RuntimeError {}
