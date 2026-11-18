//! Function resolution: which function actually runs when a script
//! calls a name. Papyrus is stateful — the same name can mean
//! different functions in different states — and inherited, so
//! resolution walks the parent chain. This module is pure: it takes
//! the loaded scripts and returns the function, with no VM state.
//!
//! Resolution order for a call to `F` on an instance of script `S`
//! in state `T` (design doc §4):
//!
//! 1. `S`'s state `T` version of `F`.
//! 2. The parent script's state `T` version of `F`.
//! 3. `S`'s empty-state version of `F`.
//! 4. The parent script's empty-state version of `F`.
//!
//! ...continuing up the chain (grandparent state `T`, grandparent
//! empty, ...) until a version is found or the chain ends. Papyrus
//! names are case-insensitive throughout.

use dovah_pex::{Function, PexFile, PexObject, Property, Variable};
use std::collections::HashMap;
use std::rc::Rc;

/// The loaded script definitions, indexed by script name
/// (lowercased — Papyrus names are case-insensitive).
#[derive(Debug, Default)]
pub struct ScriptRegistry {
    objects: HashMap<String, Rc<PexObject>>,
}

impl ScriptRegistry {
    pub fn new() -> Self {
        ScriptRegistry::default()
    }

    /// Load every object in a parsed `.pex` file.
    pub fn add_file(&mut self, file: &PexFile) {
        for object in &file.objects {
            self.add_object(object.clone());
        }
    }

    /// Load one script definition.
    pub fn add_object(&mut self, object: PexObject) {
        self.objects
            .insert(object.name.to_ascii_lowercase(), Rc::new(object));
    }

    /// Look up a script definition by name (case-insensitive).
    pub fn object(&self, name: &str) -> Option<&PexObject> {
        self.objects.get(&name.to_ascii_lowercase()).map(Rc::as_ref)
    }

    /// The script names currently loaded.
    pub fn script_names(&self) -> Vec<&str> {
        self.objects.values().map(|o| o.name.as_str()).collect()
    }

    pub fn len(&self) -> usize {
        self.objects.len()
    }

    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }

    /// True when script `child` is `parent` or extends it, walking
    /// the parent chain. Case-insensitive. A script with no loaded
    /// parent (e.g. the built-in `Script` base, which has no PEX)
    /// ends the chain.
    pub fn is_subtype(&self, child: &str, parent: &str) -> bool {
        if child.eq_ignore_ascii_case(parent) {
            return true;
        }
        let mut current = child.to_string();
        while let Some(object) = self.object(&current) {
            if object.parent.is_empty() {
                return false;
            }
            if object.parent.eq_ignore_ascii_case(parent) {
                return true;
            }
            current = object.parent.clone();
        }
        false
    }

    /// The parent chain of a script, starting with the script
    /// itself: `[S, parent(S), parent(parent(S)), ...]`, stopping
    /// at the first script whose parent is not loaded (the built-in
    /// `Script` base has no PEX file).
    pub fn chain(&self, script: &str) -> Vec<&PexObject> {
        let mut out = Vec::new();
        let mut current = script.to_string();
        while let Some(object) = self.object(&current) {
            let parent = object.parent.clone();
            out.push(object);
            if parent.is_empty() {
                break;
            }
            current = parent;
        }
        out
    }
}

/// A resolved function: the function itself, and the script object
/// that defined it (which may be a parent of the instance's script).
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved<'a> {
    pub owner: &'a PexObject,
    pub function: &'a Function,
}

/// Resolve a call to `name` on an instance of `script` in `state`,
/// following the order in the module docs. Returns the function and
/// the object that defined it, or `None` when no script in the
/// chain defines it.
pub fn resolve_function<'a>(
    registry: &'a ScriptRegistry,
    script: &str,
    state: &str,
    name: &str,
) -> Option<Resolved<'a>> {
    resolve_from(registry, script, state, name, false)
}

/// Resolve a `callparent`: the same search, but starting from the
/// *parent* of `script` — the caller's own versions are skipped.
pub fn resolve_parent_function<'a>(
    registry: &'a ScriptRegistry,
    script: &str,
    state: &str,
    name: &str,
) -> Option<Resolved<'a>> {
    resolve_from(registry, script, state, name, true)
}

fn resolve_from<'a>(
    registry: &'a ScriptRegistry,
    script: &str,
    state: &str,
    name: &str,
    skip_self: bool,
) -> Option<Resolved<'a>> {
    let chain = registry.chain(script);
    let chain: &[&PexObject] = if skip_self {
        if chain.is_empty() {
            &[]
        } else {
            &chain[1..]
        }
    } else {
        &chain
    };
    // State versions first, walking up the chain; then empty-state
    // versions, walking up the chain again.
    if !state.is_empty() {
        for object in chain {
            if let Some(function) = find_function(object, state, name) {
                return Some(Resolved {
                    owner: object,
                    function,
                });
            }
        }
    }
    for object in chain {
        if let Some(function) = find_function(object, "", name) {
            return Some(Resolved {
                owner: object,
                function,
            });
        }
    }
    None
}

/// Find a function in one object, matching state and function names
/// case-insensitively (Papyrus names are case-insensitive; the
/// parser's own lookups are exact, so this is done here).
fn find_function<'a>(object: &'a PexObject, state: &str, name: &str) -> Option<&'a Function> {
    object
        .states
        .iter()
        .find(|s| s.name.eq_ignore_ascii_case(state))
        .and_then(|s| {
            s.functions
                .iter()
                .find(|f| f.name.eq_ignore_ascii_case(name))
                .map(|f| &f.function)
        })
}

/// Find a script-level variable by name, walking the parent chain.
/// Returns the variable and the object that declared it.
pub fn resolve_variable<'a>(
    registry: &'a ScriptRegistry,
    script: &str,
    name: &str,
) -> Option<(&'a PexObject, &'a Variable)> {
    for object in registry.chain(script) {
        if let Some(variable) = object
            .variables
            .iter()
            .find(|v| v.name.eq_ignore_ascii_case(name))
        {
            return Some((object, variable));
        }
    }
    None
}

/// Find a property by name, walking the parent chain.
pub fn resolve_property<'a>(
    registry: &'a ScriptRegistry,
    script: &str,
    name: &str,
) -> Option<(&'a PexObject, &'a Property)> {
    for object in registry.chain(script) {
        if let Some(property) = object
            .properties
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(name))
        {
            return Some((object, property));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use dovah_pex::{Function, NamedFunction, State};

    fn function() -> Function {
        Function {
            return_type: "None".to_string(),
            docstring: String::new(),
            user_flags: 0,
            flags: 0,
            params: vec![],
            locals: vec![],
            instructions: vec![],
        }
    }

    fn object(name: &str, parent: &str, states: Vec<State>) -> PexObject {
        PexObject {
            name: name.to_string(),
            parent: parent.to_string(),
            docstring: String::new(),
            user_flags: 0,
            auto_state: String::new(),
            variables: vec![],
            properties: vec![],
            states,
        }
    }

    fn state(name: &str, functions: &[&str]) -> State {
        State {
            name: name.to_string(),
            functions: functions
                .iter()
                .map(|f| NamedFunction {
                    name: f.to_string(),
                    function: function(),
                })
                .collect(),
        }
    }

    fn registry() -> ScriptRegistry {
        let mut registry = ScriptRegistry::new();
        // Parent defines OnInit (empty state) and Open (empty).
        registry.add_object(object(
            "Base",
            "Script",
            vec![state("", &["OnInit", "Open"])],
        ));
        // Child overrides Open in the empty state and in state
        // "Busy", and adds Close only in state "Busy".
        registry.add_object(object(
            "Child",
            "Base",
            vec![state("", &["Open"]), state("Busy", &["Open", "Close"])],
        ));
        registry
    }

    #[test]
    fn resolution_order_state_then_parent_then_empty() {
        let registry = registry();
        // Child in state Busy: its own Busy Open wins.
        let r = resolve_function(&registry, "Child", "Busy", "Open").unwrap();
        assert_eq!(r.owner.name, "Child");
        // Child in state Busy, Close: only the Busy version exists.
        let r = resolve_function(&registry, "Child", "Busy", "Close").unwrap();
        assert_eq!(r.owner.name, "Child");
        // Child in the empty state: its own empty Open wins over
        // Base's.
        let r = resolve_function(&registry, "Child", "", "Open").unwrap();
        assert_eq!(r.owner.name, "Child");
        // Child in state Busy, OnInit: no Busy version anywhere, so
        // Base's empty-state OnInit is found.
        let r = resolve_function(&registry, "Child", "Busy", "OnInit").unwrap();
        assert_eq!(r.owner.name, "Base");
        // Nothing defines Dance.
        assert!(resolve_function(&registry, "Child", "Busy", "Dance").is_none());
        // Names are case-insensitive.
        let r = resolve_function(&registry, "child", "busy", "open").unwrap();
        assert_eq!(r.owner.name, "Child");
    }

    #[test]
    fn parent_resolution_skips_self() {
        let registry = registry();
        // Parent.Open from Child: Child's own Open is skipped, so
        // Base's is found.
        let r = resolve_parent_function(&registry, "Child", "", "Open").unwrap();
        assert_eq!(r.owner.name, "Base");
        // Parent.Close from Child in state Busy: Base has no Close.
        assert!(resolve_parent_function(&registry, "Child", "Busy", "Close").is_none());
    }

    #[test]
    fn subtype_walks_the_chain() {
        let registry = registry();
        assert!(registry.is_subtype("Child", "Base"));
        assert!(registry.is_subtype("Child", "child"));
        assert!(registry.is_subtype("Base", "Base"));
        assert!(!registry.is_subtype("Base", "Child"));
        // "Script" is the unloaded base: Base extends it, but the
        // chain ends there (no PEX for Script itself).
        assert!(registry.is_subtype("Base", "Script"));
        assert!(registry.is_subtype("Child", "Script"));
        assert!(!registry.is_subtype("Child", "Actor"));
    }
}
