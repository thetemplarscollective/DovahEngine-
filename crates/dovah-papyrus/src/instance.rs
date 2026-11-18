//! Script instances: a script attached to one game object, with its
//! own variable values, property values, and current state.
//!
//! A **script** is the compiled definition (`PexObject`). An
//! **instance** is that script on one form: the same script on two
//! doors is two instances with separate variables. The store is
//! keyed by (form ID, script name), and instances are addressed
//! inside the VM by a small numeric [`InstanceId`] — that is what a
//! Papyrus object value holds.

use crate::value::{RuntimeValue, StringInterner};
use dovah_pex::{PexObject, Value as PexValue};
use std::collections::HashMap;

/// A handle to one script instance. Small and `Copy`, so object
/// values can carry it cheaply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InstanceId(pub u32);

/// One script attached to one form.
#[derive(Debug)]
pub struct ScriptInstance {
    pub id: InstanceId,
    /// Script name, as written in the PEX (original casing).
    pub script: String,
    /// The form this instance is attached to.
    pub form: u32,
    /// Script variables by name (original casing as declared).
    /// Includes auto-property backing variables — an auto
    /// property *is* its hidden variable, so reading the property
    /// reads this map.
    pub variables: HashMap<String, RuntimeValue>,
    /// Current state name; `""` is the empty state.
    pub state: String,
}

impl ScriptInstance {
    /// Read a variable by name (case-insensitive, as in Papyrus).
    pub fn get(&self, name: &str) -> Option<&RuntimeValue> {
        self.variables
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v)
    }

    /// Write a variable by name (case-insensitive match; keeps the
    /// originally declared casing as the key).
    pub fn set(&mut self, name: &str, value: RuntimeValue) {
        if let Some(key) = self
            .variables
            .keys()
            .find(|k| k.eq_ignore_ascii_case(name))
            .cloned()
        {
            self.variables.insert(key, value);
        } else {
            self.variables.insert(name.to_string(), value);
        }
    }
}

/// The instance store: every live script instance, keyed by ID and
/// by (form ID, script name).
#[derive(Debug, Default)]
pub struct InstanceStore {
    instances: HashMap<InstanceId, ScriptInstance>,
    by_form_script: HashMap<(u32, String), InstanceId>,
    next_id: u32,
}

impl InstanceStore {
    pub fn new() -> Self {
        InstanceStore::default()
    }

    /// Allocate a new instance for `script` on `form`, with its
    /// variables at their PEX defaults and its state at the
    /// script's auto state. Parent-chain variables are included:
    /// a child instance holds its parents' variables too (parents
    /// first, so a same-named child variable wins — the compiler
    /// forbids that, but the store stays sane if it happens).
    ///
    /// The instance is inserted into the store *before* any VMAD
    /// values are applied, so a property that points back at the
    /// same form resolves to this instance instead of recursing.
    pub fn create(
        &mut self,
        chain: &[&PexObject],
        form: u32,
        interner: &mut StringInterner,
    ) -> InstanceId {
        let id = InstanceId(self.next_id);
        self.next_id += 1;
        let script = chain.first().map(|o| o.name.clone()).unwrap_or_default();
        let auto_state = chain
            .first()
            .map(|o| o.auto_state.clone())
            .unwrap_or_default();
        let mut variables = HashMap::new();
        // Parents first, then the child, so the child's defaults
        // win on a name collision.
        for object in chain.iter().rev() {
            for variable in &object.variables {
                variables.insert(
                    variable.name.clone(),
                    pex_default_to_runtime(&variable.type_name, &variable.default, interner),
                );
            }
        }
        // `::state` is the compiler's implicit state variable: it
        // is not declared in the PEX, but every script's
        // compiler-generated `GotoState` assigns it and
        // `GetState` returns it. Initialize it to the auto state
        // so both work from the start.
        variables.insert(
            "::state".to_string(),
            RuntimeValue::Str(interner.intern(&auto_state)),
        );
        let key = (form, script.to_ascii_lowercase());
        self.by_form_script.insert(key, id);
        self.instances.insert(
            id,
            ScriptInstance {
                id,
                script,
                form,
                variables,
                state: auto_state,
            },
        );
        id
    }

    pub fn get(&self, id: InstanceId) -> Option<&ScriptInstance> {
        self.instances.get(&id)
    }

    pub fn get_mut(&mut self, id: InstanceId) -> Option<&mut ScriptInstance> {
        self.instances.get_mut(&id)
    }

    /// Find the instance of `script` on `form`, if one exists.
    pub fn find(&self, form: u32, script: &str) -> Option<InstanceId> {
        self.by_form_script
            .get(&(form, script.to_ascii_lowercase()))
            .copied()
    }

    /// All instances attached to one form.
    pub fn on_form(&self, form: u32) -> Vec<InstanceId> {
        self.instances
            .values()
            .filter(|i| i.form == form)
            .map(|i| i.id)
            .collect()
    }

    /// All instances of one script, on any form.
    pub fn of_script(&self, script: &str) -> Vec<InstanceId> {
        let lower = script.to_ascii_lowercase();
        self.instances
            .values()
            .filter(|i| i.script.eq_ignore_ascii_case(&lower))
            .map(|i| i.id)
            .collect()
    }

    pub fn len(&self) -> usize {
        self.instances.len()
    }

    pub fn is_empty(&self) -> bool {
        self.instances.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &ScriptInstance> {
        self.instances.values()
    }
}

/// Convert a PEX variable default (a parser `Value`) into a runtime
/// value. An explicit literal wins as-is; string defaults are
/// interned. A default of `None` means "no explicit default", so
/// the variable starts at its type's zero value (0, 0.0, false,
/// `""` — and `None` for objects, arrays, and `Var`).
///
/// The zero value matters: `defaultOnEnter.OnInit` does `iadd
/// targetCountTotal, 1` on a variable whose PEX default is `None`,
/// which only works if the variable starts at 0, not `None`
/// (proven in M7.1 — the game plainly works, so the engine must
/// zero value-typed variables).
fn pex_default_to_runtime(
    type_name: &str,
    default: &PexValue,
    interner: &mut StringInterner,
) -> RuntimeValue {
    match default {
        PexValue::None => RuntimeValue::default_for_type(type_name, interner),
        PexValue::Identifier(_) => RuntimeValue::None,
        PexValue::Str(s) => RuntimeValue::Str(interner.intern(s)),
        PexValue::Int(n) => RuntimeValue::Int(*n),
        PexValue::Float(f) => RuntimeValue::Float(*f),
        PexValue::Bool(b) => RuntimeValue::Bool(*b),
    }
}
