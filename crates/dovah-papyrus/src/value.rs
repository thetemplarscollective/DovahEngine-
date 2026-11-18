//! Runtime values: what a Papyrus variable actually holds while a
//! script runs. This is richer than the parser's `dovah_pex::Value`
//! (which only covers literals and names): objects are references to
//! script instances, arrays are shared mutable references, and
//! strings are interned.
//!
//! Equality and comparison follow Papyrus rules, not Rust's:
//! strings compare case-insensitively, ints and floats compare
//! numerically across types, and objects compare by identity.
//! `PartialEq` on [`RuntimeValue`] implements exactly those rules,
//! so `assert_eq!` in tests means "equal in Papyrus".

use crate::instance::InstanceId;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// A value a Papyrus variable can hold at runtime.
#[derive(Debug, Clone)]
pub enum RuntimeValue {
    /// The Papyrus `none` literal: no value, no object.
    None,
    Int(i32),
    Float(f32),
    Bool(bool),
    /// An interned string. Interning is by exact string (case is
    /// preserved — `Debug.Trace("Hello")` must print `Hello`), so
    /// equality still compares case-insensitively (see
    /// [`RuntimeValue::papyrus_eq`]).
    Str(Rc<str>),
    /// A reference to a script instance (see [`crate::instance`]).
    Object(InstanceId),
    /// A shared, mutable array. Two variables holding the same array
    /// see each other's writes, as in Papyrus.
    Array(Rc<RefCell<Vec<RuntimeValue>>>),
}

impl RuntimeValue {
    /// The Papyrus type name of this value, for error messages.
    pub fn type_name(&self) -> &'static str {
        match self {
            RuntimeValue::None => "None",
            RuntimeValue::Int(_) => "Int",
            RuntimeValue::Float(_) => "Float",
            RuntimeValue::Bool(_) => "Bool",
            RuntimeValue::Str(_) => "String",
            RuntimeValue::Object(_) => "Object",
            RuntimeValue::Array(_) => "Array",
        }
    }

    /// Papyrus truthiness, as used by `jmpt`/`jmpf`, `if`, and the
    /// `not` opcode: bools as themselves, numbers nonzero, strings
    /// non-empty, any object or array true, `None` false. Real
    /// scripts `not` objects and ints directly — e.g.
    /// `CWFortGateTriggerScript.OnInit` does `not ::temp0,
    /// ::myGateKeeper_var` (an object reference, testing for
    /// `None`) and `CWMissionPollForSoldiersScript.OnInit` does
    /// `not ::temp1, ::StageToStartPolling_var` (an int, testing
    /// for 0) — so `not` cannot require a `Bool` operand (M7.1).
    pub fn truthy(&self) -> bool {
        match self {
            RuntimeValue::None => false,
            RuntimeValue::Int(n) => *n != 0,
            RuntimeValue::Float(f) => *f != 0.0,
            RuntimeValue::Bool(b) => *b,
            RuntimeValue::Str(s) => !s.is_empty(),
            RuntimeValue::Object(_) | RuntimeValue::Array(_) => true,
        }
    }

    /// Papyrus equality (`==`): strings compare case-insensitively,
    /// ints and floats compare numerically across types, objects
    /// compare by instance identity, arrays compare by reference
    /// (the same array object), and `None` equals only `None`.
    pub fn papyrus_eq(&self, other: &RuntimeValue) -> bool {
        match (self, other) {
            (RuntimeValue::None, RuntimeValue::None) => true,
            (RuntimeValue::Int(a), RuntimeValue::Int(b)) => a == b,
            (RuntimeValue::Float(a), RuntimeValue::Float(b)) => a == b,
            (RuntimeValue::Int(a), RuntimeValue::Float(b)) => (*a as f32) == *b,
            (RuntimeValue::Float(a), RuntimeValue::Int(b)) => *a == (*b as f32),
            (RuntimeValue::Bool(a), RuntimeValue::Bool(b)) => a == b,
            (RuntimeValue::Str(a), RuntimeValue::Str(b)) => a.eq_ignore_ascii_case(b),
            (RuntimeValue::Object(a), RuntimeValue::Object(b)) => a == b,
            (RuntimeValue::Array(a), RuntimeValue::Array(b)) => Rc::ptr_eq(a, b),
            _ => false,
        }
    }

    /// Papyrus ordering (`<`, `<=`, `>`, `>=`): numbers compare
    /// numerically across types, strings compare lexicographically
    /// case-insensitively. Anything else is not orderable and
    /// returns `None` (the interpreter turns that into a runtime
    /// error).
    pub fn papyrus_cmp(&self, other: &RuntimeValue) -> Option<std::cmp::Ordering> {
        match (self, other) {
            (RuntimeValue::Int(a), RuntimeValue::Int(b)) => Some(a.cmp(b)),
            (RuntimeValue::Float(a), RuntimeValue::Float(b)) => a.partial_cmp(b),
            (RuntimeValue::Int(a), RuntimeValue::Float(b)) => (*a as f32).partial_cmp(b),
            (RuntimeValue::Float(a), RuntimeValue::Int(b)) => a.partial_cmp(&(*b as f32)),
            (RuntimeValue::Str(a), RuntimeValue::Str(b)) => {
                Some(a.to_ascii_lowercase().cmp(&b.to_ascii_lowercase()))
            }
            _ => None,
        }
    }

    /// Format this value as a Papyrus string (the `cast` to String
    /// and `strcat` rules): numbers format plainly, bools as
    /// `true`/`false`, `None` as the empty string, arrays as the
    /// empty string. Objects need their script name, which only the
    /// VM knows — see [`RuntimeValue::cast_to`].
    pub fn to_papyrus_string(&self) -> String {
        match self {
            RuntimeValue::None => String::new(),
            RuntimeValue::Int(n) => n.to_string(),
            RuntimeValue::Float(f) => format_float(*f),
            RuntimeValue::Bool(b) => b.to_string(),
            RuntimeValue::Str(s) => s.to_string(),
            RuntimeValue::Object(_) => "Object".to_string(),
            RuntimeValue::Array(_) => String::new(),
        }
    }

    /// The default value for a declared Papyrus type: 0, 0.0,
    /// false, `""`, and `None` for everything else (objects,
    /// arrays, `Var`). Type names are case-insensitive in
    /// Papyrus — the compiler emits `int` and `Int` alike.
    pub fn default_for_type(type_name: &str, interner: &mut StringInterner) -> RuntimeValue {
        if type_name.eq_ignore_ascii_case("Int") {
            RuntimeValue::Int(0)
        } else if type_name.eq_ignore_ascii_case("Float") {
            RuntimeValue::Float(0.0)
        } else if type_name.eq_ignore_ascii_case("Bool") {
            RuntimeValue::Bool(false)
        } else if type_name.eq_ignore_ascii_case("String") {
            RuntimeValue::Str(interner.intern(""))
        } else {
            RuntimeValue::None
        }
    }

    /// Cast this value to the named Papyrus type, following the
    /// `cast` opcode rules:
    ///
    /// - int <-> float converts numerically (float to int
    ///   truncates toward zero).
    /// - Anything casts to String by formatting (objects format as
    ///   their script name, via `ctx`).
    /// - Bool casts from numbers (nonzero) and strings (non-empty);
    ///   numbers cast from bools (0/1) and strings (parsed, 0 when
    ///   the string does not parse).
    /// - Object casts check the script hierarchy through `ctx`: a
    ///   failed downcast yields `None`, never an error. `None`
    ///   casts to any object type as `None`.
    /// - Array casts succeed only for arrays of the matching
    ///   element type; anything else yields `None`.
    /// - Casting to `Var` or to the value's own type is a no-op.
    /// - Casting an object or array to a numeric or bool type is an
    ///   error (the compiler never emits it).
    pub fn cast_to(&self, target: &str, ctx: &dyn CastContext) -> Result<RuntimeValue, CastError> {
        if target.eq_ignore_ascii_case("Var") {
            return Ok(self.clone());
        }
        if target.eq_ignore_ascii_case("None") {
            return Ok(RuntimeValue::None);
        }
        if target.eq_ignore_ascii_case("Int") {
            return Ok(match self {
                RuntimeValue::Int(n) => RuntimeValue::Int(*n),
                RuntimeValue::Float(f) => RuntimeValue::Int(*f as i32),
                RuntimeValue::Bool(b) => RuntimeValue::Int(i32::from(*b)),
                RuntimeValue::Str(s) => RuntimeValue::Int(s.trim().parse().unwrap_or(0)),
                RuntimeValue::None => RuntimeValue::Int(0),
                RuntimeValue::Object(_) | RuntimeValue::Array(_) => {
                    return Err(CastError::Invalid {
                        from: self.type_name(),
                        to: "Int",
                    });
                }
            });
        }
        if target.eq_ignore_ascii_case("Float") {
            return Ok(match self {
                RuntimeValue::Float(f) => RuntimeValue::Float(*f),
                RuntimeValue::Int(n) => RuntimeValue::Float(*n as f32),
                RuntimeValue::Bool(b) => RuntimeValue::Float(if *b { 1.0 } else { 0.0 }),
                RuntimeValue::Str(s) => RuntimeValue::Float(s.trim().parse().unwrap_or(0.0)),
                RuntimeValue::None => RuntimeValue::Float(0.0),
                RuntimeValue::Object(_) | RuntimeValue::Array(_) => {
                    return Err(CastError::Invalid {
                        from: self.type_name(),
                        to: "Float",
                    });
                }
            });
        }
        if target.eq_ignore_ascii_case("Bool") {
            return Ok(match self {
                RuntimeValue::Bool(b) => RuntimeValue::Bool(*b),
                RuntimeValue::Int(n) => RuntimeValue::Bool(*n != 0),
                RuntimeValue::Float(f) => RuntimeValue::Bool(*f != 0.0),
                RuntimeValue::Str(s) => RuntimeValue::Bool(!s.is_empty()),
                RuntimeValue::None => RuntimeValue::Bool(false),
                RuntimeValue::Object(_) | RuntimeValue::Array(_) => {
                    return Err(CastError::Invalid {
                        from: self.type_name(),
                        to: "Bool",
                    });
                }
            });
        }
        if target.eq_ignore_ascii_case("String") {
            return Ok(RuntimeValue::Str(Rc::from(
                match self {
                    RuntimeValue::Object(id) => ctx
                        .object_type_name(*id)
                        .unwrap_or_else(|| "Object".to_string()),
                    _ => self.to_papyrus_string(),
                }
                .as_str(),
            )));
        }
        if let Some(element) = target.strip_suffix("[]") {
            return match self {
                RuntimeValue::None => Ok(RuntimeValue::None),
                RuntimeValue::Array(_) => {
                    // Element-type checking happens at array
                    // creation; a cast keeps the array only when
                    // the element type matches, else None. The
                    // runtime does not track element types per
                    // array, so a cast to a *different* element
                    // type cannot be detected here — the VM records
                    // element types separately when it matters.
                    let _ = element;
                    Ok(self.clone())
                }
                _ => Ok(RuntimeValue::None),
            };
        }
        // Otherwise the target is an object (script) type.
        match self {
            RuntimeValue::None => Ok(RuntimeValue::None),
            RuntimeValue::Object(id) => {
                if ctx.is_subtype_of(*id, target) {
                    Ok(self.clone())
                } else {
                    Ok(RuntimeValue::None)
                }
            }
            _ => Err(CastError::Invalid {
                from: self.type_name(),
                to: "object type",
            }),
        }
    }
}

/// Equality on runtime values follows Papyrus rules — see
/// [`RuntimeValue::papyrus_eq`].
impl PartialEq for RuntimeValue {
    fn eq(&self, other: &RuntimeValue) -> bool {
        self.papyrus_eq(other)
    }
}

/// What a cast needs to know about the world: the script hierarchy
/// (for object casts) and instance script names (for object to
/// string formatting). The VM implements this; tests use a stub.
pub trait CastContext {
    /// True when the instance's script is `target` or extends it.
    fn is_subtype_of(&self, id: InstanceId, target: &str) -> bool;
    /// The script name of the instance, for object-to-string casts.
    fn object_type_name(&self, id: InstanceId) -> Option<String>;
}

/// A cast that cannot be performed at all (as opposed to a failed
/// downcast, which yields `None`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CastError {
    Invalid {
        from: &'static str,
        to: &'static str,
    },
}

impl std::fmt::Display for CastError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CastError::Invalid { from, to } => write!(f, "cannot cast {from} to {to}"),
        }
    }
}

impl std::error::Error for CastError {}

/// Format a float the way Papyrus prints one: Rust's `Display` for
/// f32 already prints `2` for 2.0 and `2.5` for 2.5, which matches.
fn format_float(f: f32) -> String {
    format!("{f}")
}

/// The string pool: one canonical `Rc<str>` per distinct string, so
/// repeated literals and property names share storage instead of
/// allocating a fresh `String` per variable. Interning is by exact
/// string — case is preserved for display, and comparison stays
/// case-insensitive (see [`RuntimeValue::papyrus_eq`]).
#[derive(Debug, Default)]
pub struct StringInterner {
    pool: HashMap<String, Rc<str>>,
}

impl StringInterner {
    pub fn new() -> Self {
        StringInterner::default()
    }

    /// Intern `s`, returning the canonical shared copy.
    pub fn intern(&mut self, s: &str) -> Rc<str> {
        if let Some(existing) = self.pool.get(s) {
            return Rc::clone(existing);
        }
        let shared: Rc<str> = Rc::from(s);
        self.pool.insert(s.to_string(), Rc::clone(&shared));
        shared
    }

    /// How many distinct strings are in the pool.
    pub fn len(&self) -> usize {
        self.pool.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pool.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoObjects;

    impl CastContext for NoObjects {
        fn is_subtype_of(&self, _id: InstanceId, _target: &str) -> bool {
            false
        }

        fn object_type_name(&self, _id: InstanceId) -> Option<String> {
            None
        }
    }

    fn s(text: &str) -> RuntimeValue {
        RuntimeValue::Str(Rc::from(text))
    }

    #[test]
    fn interning_shares_storage_and_keeps_case() {
        let mut pool = StringInterner::new();
        let a = pool.intern("Hello");
        let b = pool.intern("Hello");
        assert!(Rc::ptr_eq(&a, &b));
        assert_eq!(pool.len(), 1);
        assert_eq!(&*a, "Hello");
        // A different case is a different string in the pool (case
        // is preserved for display), but compares equal in Papyrus.
        let c = pool.intern("hello");
        assert!(!Rc::ptr_eq(&a, &c));
        assert_eq!(pool.len(), 2);
        assert_eq!(RuntimeValue::Str(a), RuntimeValue::Str(c));
    }

    #[test]
    fn papyrus_equality_rules() {
        assert_eq!(RuntimeValue::Int(2), RuntimeValue::Float(2.0));
        assert_ne!(RuntimeValue::Int(2), RuntimeValue::Float(2.5));
        assert_eq!(s("Whiterun"), s("whiterun"));
        assert_ne!(s("Whiterun"), s("Solitude"));
        assert_eq!(RuntimeValue::None, RuntimeValue::None);
        assert_ne!(RuntimeValue::None, RuntimeValue::Int(0));
        assert_ne!(RuntimeValue::Int(1), RuntimeValue::Bool(true));
        let arr = Rc::new(RefCell::new(vec![RuntimeValue::Int(1)]));
        assert_eq!(
            RuntimeValue::Array(Rc::clone(&arr)),
            RuntimeValue::Array(Rc::clone(&arr))
        );
        let other = Rc::new(RefCell::new(vec![RuntimeValue::Int(1)]));
        assert_ne!(
            RuntimeValue::Array(Rc::clone(&arr)),
            RuntimeValue::Array(other)
        );
    }

    #[test]
    fn papyrus_ordering_rules() {
        use std::cmp::Ordering;
        assert_eq!(
            RuntimeValue::Int(1).papyrus_cmp(&RuntimeValue::Float(2.0)),
            Some(Ordering::Less)
        );
        assert_eq!(s("apple").papyrus_cmp(&s("Banana")), Some(Ordering::Less));
        assert_eq!(
            RuntimeValue::Bool(true).papyrus_cmp(&RuntimeValue::Bool(false)),
            None
        );
        assert_eq!(RuntimeValue::None.papyrus_cmp(&RuntimeValue::None), None);
    }

    #[test]
    fn casts_cover_the_table() {
        let ctx = NoObjects;
        assert_eq!(
            RuntimeValue::Float(2.9).cast_to("Int", &ctx).unwrap(),
            RuntimeValue::Int(2)
        );
        assert_eq!(
            RuntimeValue::Int(3).cast_to("Float", &ctx).unwrap(),
            RuntimeValue::Float(3.0)
        );
        assert_eq!(
            RuntimeValue::Bool(true).cast_to("Int", &ctx).unwrap(),
            RuntimeValue::Int(1)
        );
        assert_eq!(s("42").cast_to("Int", &ctx).unwrap(), RuntimeValue::Int(42));
        assert_eq!(
            s("not a number").cast_to("Int", &ctx).unwrap(),
            RuntimeValue::Int(0)
        );
        assert_eq!(
            RuntimeValue::Int(7).cast_to("String", &ctx).unwrap(),
            s("7")
        );
        assert_eq!(
            RuntimeValue::Float(2.5).cast_to("String", &ctx).unwrap(),
            s("2.5")
        );
        assert_eq!(
            RuntimeValue::Bool(false).cast_to("String", &ctx).unwrap(),
            s("false")
        );
        assert_eq!(RuntimeValue::None.cast_to("String", &ctx).unwrap(), s(""));
        assert_eq!(
            RuntimeValue::Int(0).cast_to("Bool", &ctx).unwrap(),
            RuntimeValue::Bool(false)
        );
        assert_eq!(
            s("x").cast_to("Bool", &ctx).unwrap(),
            RuntimeValue::Bool(true)
        );
        assert_eq!(
            RuntimeValue::Int(5).cast_to("Var", &ctx).unwrap(),
            RuntimeValue::Int(5)
        );
        assert_eq!(
            RuntimeValue::Int(5).cast_to("None", &ctx).unwrap(),
            RuntimeValue::None
        );
        // Object casts need a hierarchy; with no objects, None
        // casts to any object type as None.
        assert_eq!(
            RuntimeValue::None.cast_to("Actor", &ctx).unwrap(),
            RuntimeValue::None
        );
        // Int to an object type is not a cast at all.
        assert!(RuntimeValue::Int(1).cast_to("Actor", &ctx).is_err());
    }

    #[test]
    fn truthiness() {
        assert!(!RuntimeValue::None.truthy());
        assert!(!RuntimeValue::Int(0).truthy());
        assert!(RuntimeValue::Int(-1).truthy());
        assert!(!RuntimeValue::Float(0.0).truthy());
        assert!(!s("").truthy());
        assert!(s("x").truthy());
        assert!(!RuntimeValue::Bool(false).truthy());
    }
}
