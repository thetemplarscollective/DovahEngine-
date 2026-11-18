# Melee: why zero hits landed on the phone (v0.4.13)

Read-only diagnosis, 2026-11-17. No code was changed. Combat feature work
is halted; this is diagnosis only.

## The headline

The v0.4.13 phone logs prove **zero hits landed** — and nothing else. Both
runs (`gameplay/dovahengine-startup-v0413-current-20261117.log`,
`gameplay/dovahengine-startup-v0413-prev-20261117.log`) contain zero
`Hit ...` / `Player killed ...` lines, while the combat system itself built
fine (combatants built: 3 inn / 9 exterior / 2 Sven's house in the current
run, 3 / 9 in the prev run). On desktop the same code kills Orgnar in 19
swings (`melee_after.log`: 19 Hit/killed lines).

In v0.4.13 a press that does NOT land a hit logs **nothing**. There are
three silent swallow paths (whiff, menu gate, cooldown — §3), so the logs
cannot distinguish "never pressed" from "pressed and swallowed". The press
logging Cortana is adding (already in the tree — `PressLog` in touch.rs,
`log_attack_presses` in combat.rs) closes exactly this gap; §5 is the
decoding table for the next phone log.

**Most likely cause (H1): presses happened and died in the silent paths —
chiefly whiffs (1.5 m reach, ±45° arc; Orgnar is unhittable from the
customer side of the counter) and the menu gate (Dylan spent much of both
runs in dialogue/inventory).** The press pipeline itself is exonerated by
the USE control (§2).

## 1. The press path, traced frame by frame

The full path for a phone ATTACK tap:

1. **PreUpdate, frame N** — Bevy's `touch_screen_input_system`
   (`bevy_input-0.20.1/src/touch.rs`, registered in `PreUpdate` at
   `bevy_input/src/lib.rs:165`) clears `just_pressed`, then processes the
   queued `TouchInput` events. The tap's `TouchPhase::Started` lands in
   `just_pressed` for this frame only.
2. **Update, frame N** — `melee_attack` (combat.rs) runs **before**
   `track_touches` (see ordering below) and reads the stale flag
   (`attack_right_pressed = false`). No attack.
3. **Update, frame N** — `track_touches` (touch.rs:691) resets the whole
   state (`*input = TouchInputState::default()`, touch.rs:708), then the
   `iter_just_pressed()` loop (touch.rs:769) hit-tests each new touch's
   start position: a tap starting on the ATTACK R button sets
   `attack_right_pressed = true` (touch.rs:784-786); ATTACK L likewise
   (touch.rs:790-792). The hit radius is 15% larger than the drawn button
   (`button_radius`, touch.rs).
4. **PreUpdate, frame N+1** — `just_pressed` is cleared; the tap is now
   just "pressed".
5. **Update, frame N+1** — `melee_attack` reads `attack_right_pressed =
   true` and proceeds (gates → cooldown → bow check → range check).
6. **Update, frame N+1** — `track_touches` resets the flag to false.

So the flag is delivered to `melee_attack` **exactly once, one frame after
the tap**. The mechanism is sound — provided the ordering in step 2/5
holds. It does, by insertion order:

- `build_windowed_app` (lib.rs, the phone path) adds `CombatPlugin` at
  lib.rs:983 and `TouchControlsPlugin` at lib.rs:994 (`#[cfg(target_os =
  "android")]`, added after combat/archery/ruffle).
- Both systems are in `Update` and conflict on `TouchInputState`
  (`track_touches` takes `ResMut`, `melee_attack` takes
  `Option<Res<..>>`). Bevy orders conflicting systems by insertion order,
  so `melee_attack` runs before `track_touches` every frame.
- The `PressLog` design (touch.rs:131-165, Cortana's) relies on exactly
  this: "the consumers run before `track_touches` in Update, so they
  claim a press the frame after it is recorded."

Note the asymmetry with USE: the USE consumer chain (`door_use`,
`dialogue_use`, `pickup_use`, `container_use` — lib.rs:1015) is added
**after** `TouchControlsPlugin`, so USE presses are read in the **same**
frame they are set, while ATTACK presses are read the **next** frame.
Both deliver exactly once per press; the latency difference is one frame
(~60-140 ms at phone frame rates) and is not a loss mechanism.

No system consumes the flags. `melee_attack` and `archery_fire`
(archery.rs:326) both read them read-only; `track_touches` is the only
writer. `archery_fire` returns early when no bow is equipped and cannot
steal a melee press.

## 2. The USE control — the plumbing is exonerated

The ATTACK flags go through the **identical** pipeline as the USE flag:
same `Touches` resource, same `just_pressed` loop, same one-frame reset,
same `TouchInputState`. USE demonstrably works on the phone in both
v0.4.13 runs:

- Current run: door 0x00013419 opening (+122.5s), door transition
  (+133.8s), picked up Woodcutter's Axe (+261.2s), inventory opened
  (+262.7s), door 0x0001cbb0 opening (+314.9s), dialogue opened: Sven
  (+363.6s).
- Prev run: dialogue opened: Embry (+144.3s), dialogue opened: Delphine
  (+159.7s), inventory opened (+195.7s), door 0x00013419 opening
  (+204.8s), picked up Steel Dagger (+347.0s).

Any input-layer loss (event batching at 7-16 fps, `just_pressed`
clearing, the one-frame flag lifetime, multi-touch classification) would
hit USE exactly as hard as ATTACK. USE works, so the input layer works.
A press-loss mechanism specific to ATTACK must be downstream of the flag
— and the downstream paths are exactly the three silent gates in §3.

Frame pacing does not break the flag either: at 7-17 fps a frame is
60-146 ms, and the flag's lifetime is one frame regardless of duration.
A quick tap still produces a `Started` event in some frame's event batch
(the OS queues it; Bevy processes the batch in the next PreUpdate). The
250 ms Bevy delta clamp is not a factor at these frame rates (it only
bites below 4 fps).

## 3. The three silent swallow paths in v0.4.13

`melee_attack` (combat.rs) can swallow a delivered press in three places,
and in v0.4.13 **none of them logs anything**:

### (a) The menu gate — first thing in the system

```rust
if dialogue_open || inventory_open || pause_open { return; }
```

The gate returns **before the press is even read**. Dylan spent much of
both runs in dialogue or inventory:

- Prev run, inn (~126 s InGame): dialogue with Embry (+144.3s) and
  Delphine (+159.7s), inventory (+195.7-197.6s). He was standing next to
  Embry and Delphine, talking to them — the natural moment to try the
  ATTACK button is exactly while the dialogue is open. Every such press
  is swallowed silently.
- Current run: inventory open +262.7-274.5s (equip/unequip fiddling with
  the Woodcutter's Axe), dialogue with Sven +363.6s onward.

### (b) The whiff — no target in range

`in_melee_range` (combat.rs): within **1.5 m × reach stat**
horizontally, within **±45°** of the facing direction
(`MELEE_ARC_COS`), within **2.0 m** of the eye vertically. A press with
no combatant passing all three is a whiff — and the code spends the
cooldown on a whiff ("The swing happens (cooldown spent, queue consumed)
even when it hits nothing — a whiff is still a swing", combat.rs), then
returns without logging.

The geometry makes whiffs the default outcome:

- **Orgnar is almost certainly unhittable from the customer side of the
  counter.** Orgnar's Bevy position is (-18.75, 0, -0.26) (the melee
  test teleports the player to (-17.89, 1.70, -1.09), 1.2 m from him —
  *through* the counter, ignoring collision). The counter sits under the
  overhead light at (-16.55, 4.11, 0); the player spawns near the inn
  door at x≈-14.5 and is stopped by the counter's collision at roughly
  x≈-15.9 — about **2.85 m from Orgnar, nearly twice the 1.5 m fist
  reach**. The Woodcutter's Axe Dylan equipped (current run, +267.9s /
  +271.2s / +271.7s) is a one-handed axe with reach stat 1.0 — the same
  1.5 m. For Orgnar to be hittable the player would need to stand at
  x ≤ -17.25, i.e. on the counter itself. (Geometry-derived from the
  recorded positions, not render-proven — but the margin is ~1.35 m,
  far beyond any plausible counter-edge error.)
- **Delphine and Embry are hittable only up close and facing them.**
  Dialogue in this engine is proximity-based, not aim-based (the video
  shows the "Embry" label while aiming at the fire pit), so Dylan could
  open dialogue from several metres away and press ATTACK from a
  distance that reads as "right next to them" on screen but is outside
  1.5 m — or while facing away from them (the arc check fails silently).
- **Outdoors, the 9 combatants are spread across Riverwood.** Dylan
  picked up the axe at the forge and spent the exterior session around
  the smithy; a press anywhere not within 1.5 m of a specific NPC is a
  whiff.

### (c) The cooldown — 1.0 s, spent by whiffs

Fist cooldown is 1.0 s (`swing_cooldown_secs(1.0)`); the Woodcutter's
Axe is similar. A press during the cooldown returns silently. Because a
whiff spends the cooldown, the natural "nothing happened, tap it again"
pattern eats itself: press from 2 m (whiff, 1 s cooldown) → walk closer
→ press 0.5 s later (cooldown, swallowed) → press again (maybe a hit).
At 7-10 fps the 1.0 s cooldown is 7-10 frames of dead presses.

## 4. Ranked hypotheses

1. **Whiffs + menu gate + cooldown, all silent in v0.4.13 (most
   likely).** The press pipeline works (§1-2). Presses that happened
   died in §3's silent paths: no target within 1.5 m and ±45°
   (Orgnar unhittable behind the counter; Delphine/Embry only hittable
   up close and facing), or pressed while dialogue/inventory was open,
   or pressed inside the 1.0 s cooldown a previous whiff had spent.
   Zero "Hit" lines is exactly what this hypothesis predicts.
2. **Dylan never pressed ATTACK (or pressed it only once or twice).**
   Unprovable from the v0.4.13 evidence: the 15 sampled video frames
   show no ATTACK press, and the log has no press logging at all. The
   next phone log settles this (see §5): zero `press: attack_*` lines
   means he never pressed it.
3. **The flag is lost between `track_touches` and `melee_attack`
   (unlikely — effectively ruled out).** The scheduling analysis (§1)
   shows the flag is delivered exactly once, and the USE control (§2)
   proves the identical mechanism works on the phone. The one real
   fragility: the ordering is **implicit** (insertion order), with no
   explicit `.before()`/`.after()` between `melee_attack` and
   `track_touches` — a future plugin reorder could silently break the
   delivery. Worth hardening with an explicit ordering constraint when
   combat work resumes; it is not the cause of the v0.4.13 zero.
4. **Other paths (ruled out).** The bow check only yields when a bow is
   equipped (Dylan had fists, then the Woodcutter's Axe — not a bow).
   `player.single()` has exactly one Player entity. The mouse path is
   desktop-only and OR'd with the touch path, so it cannot block it.
   The 250 ms delta clamp does not bite at 7-17 fps. `archery_fire`
   reads the flags read-only and never consumes them. Presses during
   Loading/Spawning are dropped by design (no scene to hit), but Dylan
   was watching loading screens then, not pressing ATTACK.

## 5. What the press logging will settle (next phone log)

Cortana's press logging (in the tree now, shipping in the next build)
logs one line per ATTACK press with its outcome, via `PressLog::record`
in `track_touches` + `log_attack_presses` in `melee_attack`. The
decoding table for the next phone log:

| Log line | What it proves |
|---|---|
| `press: attack_right #N -> no target in range` | He pressed; the flag reached `melee_attack`; the press whiffed. H1 confirmed, range is the cause. |
| `press: attack_right #N -> on cooldown (X.Xs left)` | He pressed during the 1.0 s cooldown. Rapid-tap pattern. |
| `press: attack_right #N -> hit NAME for D` / `-> killed NAME` | A hit landed. The mystery is closed. |
| `press: attack_right #N -> bow equipped (archery)` | The press went to archery (a bow was equipped). |
| `press: attack_right #N` (bare, no outcome) | He pressed, but `melee_attack` never claimed it — the menu gate returned before the press was read (dialogue/inventory/pause open), or the state was not InGame. Bare lines ≈ menu-open presses. |
| No `press: attack_*` lines at all | He never pressed ATTACK. H2 confirmed. |

One gap in the new logging to note: the menu-gate return still happens
before any outcome is attached, so a menu-open press logs only as a bare
line — the *reason* (dialogue vs inventory vs pause) is inferred from
the adjacent log lines (e.g. `dialogue opened: ...` / `inventory
opened`), not stated in the press line itself. If bare lines dominate
the next log, the gate should log its own outcome (a one-line change,
logging only).

## 6. Two follow-ups for when combat resumes (not this diagnosis)

- **Hit feedback is invisible.** Even a landed hit has no visible
  feedback on the phone: no swing animation, no viewmodel, no health
  bar, no hit flash — damage is log-only (documented in the melee
  milestone). After the logging settles presses vs whiffs, the next
  combat work needs *some* visible proof a press did something, or the
  next playtest will read as "no melee" even when hits land.
- **Harden the flag ordering.** Add an explicit ordering constraint
  between `melee_attack` (and `archery_fire`) and `track_touches` so
  the one-frame flag delivery does not depend on plugin insertion
  order. A one-line `.before()`/`.after()`; the PressLog design already
  assumes this order.
