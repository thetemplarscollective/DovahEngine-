//! Stub `ExternalInterfaceProvider` (M1): logs every SWF→game call
//! and answers known queries with canned values.
//!
//! The bridge, proven from the decompiled `GameDelegate.as`
//! (`interface-extract/flash-exports/scripts/*/scripts/__Packages/gfx/io/GameDelegate.as`):
//!
//! * SWF → game: `GameDelegate.call(method, params, scope, callBack)`
//!   prepends `(methodName, uid)` and invokes
//!   `ExternalInterface.call(methodName, uid, ...params)`. Ruffle
//!   delivers this to [`ExternalInterfaceProvider::call_method`] with
//!   `name = methodName` and `args = [uid, ...params]`.
//! * The response MUST arrive synchronously: `GameDelegate.call`
//!   deletes its `responseHash[uid]` entry the moment
//!   `ExternalInterface.call` returns, so an async answer finds no
//!   callback. The game answers by invoking the SWF-registered
//!   `"respond"` callback with `(uid, ...results)` *during* the call.
//!   Re-entrant invocation from inside `call_method` is a tested
//!   Ruffle pattern (`tests/tests/external_interface/mod.rs`,
//!   `do_reentry`).
//! * Game → SWF goes through [`ruffle_core::Player::call_internal_interface`]
//!   with the `"call"` callback (see [`crate::runner`]).
//!
//! The HUD's SWF→game surface is small (proven by grep over the
//! exported HUD scripts): `RegisterHUDComponents` once at init,
//! `PlaySound(name)` on objective/notification events, and
//! `GetButtonFromUserEvent(userEvent)` (with a `SetButtonName`
//! callback) when a button prompt needs its keycap name.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use ruffle_core::context::UpdateContext;
use ruffle_core::external::{ExternalInterfaceProvider, Value};

/// One SWF→game call, as logged by [`StubProvider`].
#[derive(Debug, Clone, PartialEq)]
pub struct CallRecord {
    /// The `GameDelegate` method name (e.g. `"PlaySound"`).
    pub name: String,
    /// The raw arguments Ruffle delivered. For `GameDelegate.call`
    /// invocations, `args[0]` is the numeric UID and the rest are the
    /// method's parameters.
    pub args: Vec<Value>,
}

/// Shared, readable state of a [`StubProvider`].
///
/// The provider itself is moved into Ruffle (`Box<dyn
/// ExternalInterfaceProvider>`, then `Rc`'d inside the player), so
/// the harness keeps an `Arc` clone of this state to read the log
/// back after a run.
#[derive(Debug, Default)]
pub struct ProviderState {
    /// Every SWF→game call received, in order.
    pub calls: Vec<CallRecord>,
    /// Names of the ExternalInterface callbacks the SWF registered
    /// (`"call"` / `"respond"` once `GameDelegate.initialize()` runs).
    pub callbacks_available: Vec<String>,
    /// How many synchronous `"respond"` invocations were delivered.
    pub responds_sent: u32,
}

/// The M1 stub provider: log everything, answer what can be canned.
pub struct StubProvider {
    state: Arc<Mutex<ProviderState>>,
}

impl StubProvider {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(ProviderState::default())),
        }
    }

    /// Shared handle to the provider's log. Clone it *before* moving
    /// the provider into the `PlayerBuilder`.
    pub fn state(&self) -> Arc<Mutex<ProviderState>> {
        Arc::clone(&self.state)
    }

    /// Canned PC keycap name for a Skyrim user event.
    ///
    /// `SetButtonName` takes the name and loads `<name>.png` from the
    /// menu's tag-1009 keycap exports — which Ruffle does not
    /// implement yet (feasibility §4), so in M1 the art still falls
    /// back to text. The round trip itself (synchronous `"respond"`)
    /// is what M1 proves.
    fn button_for_event(event: &str) -> &'static str {
        match event {
            "Activate" => "E",
            "Jump" => "Space",
            "Sprint" => "Left Shift",
            "Shout" => "Z",
            "Sneak" => "Left Ctrl",
            "Ready Weapon" => "R",
            "Left Attack" => "Left Mouse",
            "Right Attack" => "Right Mouse",
            "Favorites" => "Q",
            "Journal" => "J",
            "Wait" => "T",
            "QuickSave" => "F5",
            "QuickLoad" => "F9",
            "Toggle POV" => "F",
            _ => "E",
        }
    }

    /// Canned item-card object for `RequestItemCardInfo`: an Iron
    /// Dagger — the fields `ItemCard.itemInfo` reads (type 2 =
    /// `ICT_WEAPON`; `effects` must be a string, "" for
    /// unenchanted). The inventory drive selects the Iron Dagger
    /// first, so the canned card matches the highlighted entry.
    fn canned_item_card() -> Value {
        let mut item = BTreeMap::new();
        item.insert("type".to_string(), Value::Number(2.0));
        item.insert("name".to_string(), Value::String("Iron Dagger".to_string()));
        item.insert("damage".to_string(), Value::Number(4.0));
        item.insert("effects".to_string(), Value::String(String::new()));
        item.insert("poisoned".to_string(), Value::Bool(false));
        item.insert("value".to_string(), Value::Number(10.0));
        item.insert("weight".to_string(), Value::Number(2.5));
        item.insert("count".to_string(), Value::Number(1.0));
        item.insert("stolen".to_string(), Value::Bool(false));
        Value::Object(item)
    }
}

impl Default for StubProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl ExternalInterfaceProvider for StubProvider {
    fn call_method(&self, context: &mut UpdateContext<'_>, name: &str, args: &[Value]) -> Value {
        {
            let mut state = self.state.lock().expect("provider state lock");
            state.calls.push(CallRecord {
                name: name.to_string(),
                args: args.to_vec(),
            });
        }

        // `GameDelegate.call` passes the UID as args[0]; the
        // synchronous "respond" must carry it back.
        if name == "GetButtonFromUserEvent" {
            let uid = args.first().cloned().unwrap_or(Value::Undefined);
            let event = match args.get(1) {
                Some(Value::String(s)) => s.clone(),
                _ => String::new(),
            };
            let button = Self::button_for_event(&event);
            if let Some(callback) = context.external_interface.get_callback("respond") {
                callback.call(context, "respond", [uid, Value::String(button.to_string())]);
                let mut state = self.state.lock().expect("provider state lock");
                state.responds_sent += 1;
            }
            return Value::Undefined;
        }

        // `RequestItemCardInfo`: the inventory menu asks for the
        // selected item's card data when the items list shows or
        // the highlight moves. Answer synchronously with a canned
        // Iron Dagger — the same respond pattern as the button
        // lookup, but with an OBJECT payload (proves objects cross
        // the respond path, which M1's string-only round trip did
        // not).
        if name == "RequestItemCardInfo" {
            let uid = args.first().cloned().unwrap_or(Value::Undefined);
            if let Some(callback) = context.external_interface.get_callback("respond") {
                callback.call(context, "respond", [uid, Self::canned_item_card()]);
                let mut state = self.state.lock().expect("provider state lock");
                state.responds_sent += 1;
            }
            return Value::Undefined;
        }

        // `PlaySound`, `RegisterHUDComponents`, and anything else:
        // logged above, no answer. `GameDelegate.call` ignores the
        // synchronous return value, so Undefined is correct.
        Value::Undefined
    }

    fn on_callback_available(&self, name: &str) {
        let mut state = self.state.lock().expect("provider state lock");
        state.callbacks_available.push(name.to_string());
    }

    fn get_id(&self) -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn button_map_covers_common_events() {
        assert_eq!(StubProvider::button_for_event("Activate"), "E");
        assert_eq!(StubProvider::button_for_event("Jump"), "Space");
        assert_eq!(StubProvider::button_for_event("Shout"), "Z");
        assert_eq!(StubProvider::button_for_event("Journal"), "J");
        // Unknown events fall back to E rather than failing.
        assert_eq!(StubProvider::button_for_event("SomethingElse"), "E");
    }

    #[test]
    fn canned_item_card_has_the_fields_itemcard_reads() {
        // `ItemCard.itemInfo` reads type/name/damage/effects (a
        // string — "" for unenchanted)/poisoned/value/weight/count/
        // stolen. A missing `effects` would throw inside the
        // setter (`.length` of undefined).
        let Value::Object(item) = StubProvider::canned_item_card() else {
            panic!("canned item card is not an object");
        };
        assert_eq!(item["type"], Value::Number(2.0));
        assert_eq!(item["name"], Value::String("Iron Dagger".into()));
        assert_eq!(item["damage"], Value::Number(4.0));
        assert_eq!(item["effects"], Value::String(String::new()));
        assert_eq!(item["poisoned"], Value::Bool(false));
        assert_eq!(item["value"], Value::Number(10.0));
        assert_eq!(item["weight"], Value::Number(2.5));
        assert_eq!(item["count"], Value::Number(1.0));
        assert_eq!(item["stolen"], Value::Bool(false));
    }

    #[test]
    fn provider_state_starts_empty() {
        let provider = StubProvider::new();
        let state = provider.state();
        let state = state.lock().unwrap();
        assert!(state.calls.is_empty());
        assert!(state.callbacks_available.is_empty());
        assert_eq!(state.responds_sent, 0);
    }

    #[test]
    fn call_record_round_trips_through_state() {
        let provider = StubProvider::new();
        let handle = provider.state();
        {
            let mut state = handle.lock().unwrap();
            state.calls.push(CallRecord {
                name: "PlaySound".to_string(),
                args: vec![Value::Number(3.0), Value::String("UIObjectiveNew".into())],
            });
        }
        let state = provider.state.lock().unwrap();
        assert_eq!(state.calls.len(), 1);
        assert_eq!(state.calls[0].name, "PlaySound");
        assert_eq!(
            state.calls[0].args[1],
            Value::String("UIObjectiveNew".into())
        );
    }
}
