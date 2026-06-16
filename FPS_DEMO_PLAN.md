# FPS Demo Living Plan

Last updated: 2026-06-16

This document tracks the plan for a new `demos/fps` Lightyear demo focused on
3D FPS prediction, rollback, interpolation, and lag compensation. The goal is to
replace the current scattered FPS/projectile examples with one smaller,
copyable, end-to-end demo.

`demos/fps/` now exists with a `Cargo.toml`. Keep that invariant in mind for
future demo subdirectories: the workspace uses a `demos/*` member glob, so a
directory without a manifest can break Cargo workspace loading.

## Goals

- Add a new `demos/fps` crate.
- Add a new `crates/integration/ahoy` crate, published as `lightyear_ahoy`.
- Use `bevy_ahoy` for capsule character movement.
- Use Lightyear for networking, prediction, rollback, interpolation, prespawn,
  and lag-compensation plumbing.
- Include a small but useful 3D map with floor, ramps, banks, and a mantle or
  step obstacle.
- Include two weapons:
  - hitscan, with server-side lag compensation;
  - slow projectile, with local prediction for the shooter and delayed
    interpolation-timeline spawning for remote clients.
- Include enough visual/debug feedback to understand prediction corrections,
  rewind hits, and projectile spawn timing.

## Non-Goals For First Pass

- Do not port the whole `bevy_netahoy` replication stack. It uses Replicon and
  Aeronet directly; this demo should show the Lightyear way.
- Do not keep every mode from `examples/projectiles`. That example explores many
  strategies, but the demo should be a polished canonical path.
- Do not solve player-player rollback collision in the first pass. Remote player
  solid collision during prediction is a known source of correction jitter.
- Do not make rocket jumps the first milestone. Lag-compensated rockets that
  apply impulses to players are meaningfully harder than hitscan validation.

## Current Implementation Status

- [x] Added `crates/integration/ahoy` as `lightyear_ahoy`.
- [x] Added `demos/fps` as package `fps_demo` because `examples/fps` already
      owns the `fps` package name in this workspace.
- [x] Added a conservative native-input movement path using
      `AhoyUserCommand`.
- [x] Added a BEI bridge in `lightyear_ahoy::bei`: users bind Ahoy BEI actions
      while the adapter samples them into hidden `AhoyUserCommand` snapshots.
- [x] Added shared deterministic terrain colliders: floor, ramp, banks, mantle
      block, jump block, and reset platform.
- [x] Added server-spawned replicated players with owner-only prediction and
      interpolation for other clients.
- [x] Added local predicted Ahoy controller/collider insertion on clients.
- [x] Added basic first-person camera/mouse-look rendering behind `gui`.
- [x] Updated `demos/fps` to use the BEI bridge instead of reading
      `ButtonInput<KeyCode>` directly.
- [ ] Add hitscan weapon and lag-compensated server validation.
- [ ] Add slow projectile weapon and interpolation-timeline remote visuals.

Checks run after the BEI bridge update:

```text
cargo check -p lightyear_ahoy --all-features -j 4
cargo check -p fps_demo -j 4
cargo check -p fps_demo --no-default-features --features client,netcode,udp -j 4
cargo check -p fps_demo --no-default-features --features server,netcode,udp -j 4
cargo check -p lightyear --no-default-features --features std,client,server,prediction,ahoy_bei -j 4
```

## Background: Ahoy Model

`bevy_ahoy` provides a movement controller on top of Avian 3D.

KCC means "kinematic character controller". A KCC moves a character by choosing
where the capsule should go, sweeping/sliding against world colliders, and
writing the resulting movement state. It is not a fully dynamic rigid body where
forces alone determine movement. This is common for FPS movement because it gives
direct control over walking, air movement, stairs, crouch, mantle, surf ramps,
and similar gameplay.

Ahoy's important pieces are:

- `CharacterController`: the configuration component. It contains movement
  parameters such as speed, gravity, jump height, crouch height, step size, and
  acceleration.
- `AccumulatedInput`: per-tick movement intent. It stores movement axes and
  transient actions such as jump, crouch, mantle, crane, and swim-up.
- `CharacterLook`: yaw and pitch. Movement and weapon direction should use this.
- `CharacterControllerState`: persistent controller state needed for the next
  tick. This includes orientation, whether the player is grounded or crouching,
  platform velocity, mantle/crane progress, and internal timers.
- `CharacterControllerStepper::step_entity`: Ahoy's manual stepping API. It runs
  exactly one KCC step for exactly one entity.

For rollback, `CharacterControllerState` matters. Position and velocity alone
are not enough. If we restore an old position but not grounded/crouch/mantle
state and timers, the next replayed tick can diverge from the original predicted
simulation.

## Ahoy And Avian Position/Transform

Ahoy's stepper writes movement to `Transform`. Avian and Lightyear's Avian
integration use `Position` as the compact physics/network state in the existing
3D examples.

The `lightyear_ahoy` adapter should do the same thing as `bevy_netahoy` after
every KCC step:

1. Convert the Lightyear input for the current tick into Ahoy `AccumulatedInput`
   and `CharacterLook`.
2. Call `CharacterControllerStepper::step_entity(entity, fixed_delta)`.
3. Immediately copy `Transform.translation` into `Position`.

That copy should be explicit and owned by `lightyear_ahoy`. Relying on generic
Avian transform sync would make schedule ordering harder to reason about because
rollback, prediction history, physics, and frame interpolation all need a
consistent `Position` for the same tick. The demo should use the integration
crate API and should not contain the adapter itself.

## Input Strategy

Ahoy's default local input model is `bevy_enhanced_input` (BEI). Ahoy defines
input actions such as `Movement`, `Jump`, `Crouch`, `Mantle`, and `RotateCamera`.
Ahoy's input observers receive BEI `Fire<A>` events and write those values into
`AccumulatedInput`; the KCC then reads `AccumulatedInput` plus `CharacterLook`.

Lightyear is compatible with BEI through `lightyear_inputs_bei`, so BEI can be
networked. The BEI path replicates action state on action entities (`ActionOf<C>`)
and buffers/restores trigger state, action value, events, and action time for
rollback. It also re-runs BEI apply after restored inputs during rollback, which
is the key piece needed for action events to drive predicted simulation.

Updated implementation direction: make the first-class user-facing input path
use BEI, but keep the network/replay representation as a compact hidden
`AhoyUserCommand`. This matches Ahoy's binding/action model while avoiding the
need to make BEI action entities part of the core movement networking surface.

The implemented BEI bridge path looks like:

1. The demo/app adds `AhoyBeiInputPlugin::<PlayerInputContext>`.
2. The app spawns local BEI action entities with
   `ActionOf<PlayerInputContext>` and bindings for Ahoy actions such as
   `Movement`, `Jump`, `Crouch`, `Mantle`, and `RotateCamera`.
3. During each local fixed tick, BEI `Update` and `Apply` update the action
   component values.
4. `lightyear_ahoy` samples those BEI action values into
   `ActionState<AhoyUserCommand>` in `InputSystems::WriteClientInputs`.
5. Lightyear's native input plugin buffers/sends/restores the hidden command.
6. `lightyear_ahoy` writes `AccumulatedInput` and `CharacterLook` from the
   restored command.
7. `lightyear_ahoy` manually calls `CharacterControllerStepper::step_entity`.
8. `lightyear_ahoy` mirrors `Transform.translation` into `Position` before
   prediction history/correction logic observes the tick state.

The main scheduling contract is:

```text
BEI Update -> BEI Apply -> hidden AhoyUserCommand write/buffer
    -> restored AhoyUserCommand -> Ahoy KCC step
    -> Transform-to-Position mirror -> prediction history/correction
```

`lightyear_ahoy` should own the Ahoy-specific scheduling and stepping. The FPS
demo should own demo-specific action binding, weapon actions, and UI.

### Native Command Alternative

A compact native Lightyear command is still useful as a reference/fallback path,
especially because it mirrors `bevy_netahoy` and is easy to inspect in tests.
It should not be the only supported path if the BEI path works cleanly.

Proposed command shape:

```rust
pub struct AhoyUserCommand {
    pub movement: Vec2,
    pub look: Vec2,
    pub buttons: AhoyButtons,
}

pub struct AhoyButtons {
    pub jump: bool,
    pub crouch: bool,
    pub mantle: bool,
    pub crane: bool,
    pub climbdown: bool,
    pub swim_up: bool,
}
```

The native input plugin would replicate `AhoyUserCommand`. `lightyear_ahoy`
would convert that command into `AccumulatedInput` and `CharacterLook`, step the
KCC, then copy `Transform.translation` to `Position`.

Why keep this option:

- it matches Source-style user commands and `bevy_netahoy`'s model;
- movement, look, and fire buttons can live in one tick-addressed command;
- edge detection for jump/fire is explicit via previous command buttons;
- no BEI action entity prespawn/mapping is required for the core movement loop;
- rollback replay is easier to reason about because the adapter sets the whole
  Ahoy input state from the command each tick.

### Direct BEI Integration Tradeoff

Direct BEI integration would mean making `bevy_enhanced_input` action entities
the replicated/predicted input surface instead of collapsing them into a compact
movement command first.

Benefits:

- game code can keep using BEI input contexts, action modifiers, triggers, and
  rebinding as the canonical local input model;
- non-movement actions can share the same high-level action system instead of
  adding fields to an FPS-specific command struct;
- Lightyear already has `lightyear_inputs_bei`, so the integration can reuse
  existing action-state rollback support instead of inventing BEI buffering;
- if a game is already built around BEI, the adapter layer can be thinner.

Costs:

- BEI action entities become part of the networking and prediction surface,
  including prespawn/entity mapping concerns;
- the movement step still ultimately needs a deterministic per-tick
  `AccumulatedInput` and `CharacterLook`, so the adapter must choose exactly
  how action values/events collapse into one KCC command for replay;
- edge-sensitive state such as jump/mantle/fire must be restored precisely after
  rollback, otherwise replay can double-trigger or miss a trigger;
- debugging is harder because the authoritative thing being compared is spread
  across multiple action states instead of one tick command.

Updated conclusion: use a BEI-facing bridge as the primary demo path, with BEI
actions as the user binding API and hidden `AhoyUserCommand` snapshots as the
network/replay API. A raw action-entity-networked BEI mode can still be
considered later for projects that need to replicate arbitrary BEI action state
directly.

### BEI/Ahoy Schedule Risks

Current relevant scheduling:

- Bevy fixed schedule order is `FixedFirst -> FixedPreUpdate -> FixedUpdate ->
  FixedPostUpdate -> FixedLast`.
- Lightyear rollback re-runs `FixedMain` for each replay tick, so rollback
  replays `FixedPreUpdate`, `FixedUpdate`, and `FixedPostUpdate`.
- `lightyear_inputs_bei` installs the input context in `FixedPreUpdate`. On the
  client it orders `EnhancedInputSystems::Update -> BufferClientInputs ->
  EnhancedInputSystems::Apply`. During rollback, `EnhancedInputSystems::Update`
  is skipped and the action state is restored from the input buffer before
  `EnhancedInputSystems::Apply`, so BEI events can fire again for the replayed
  tick.
- On the server, received input messages update action state in
  `FixedPreUpdate` before `EnhancedInputSystems::Apply`.
- Ahoy's `AhoyInputPlugin` registers `Fire<A>` observers that write
  `AccumulatedInput`. It also runs `tick_timers` in `PreUpdate` in
  `EnhancedInputSystems::Update`, and clears transient accumulated input in
  `RunFixedMainLoop::AfterFixedMainLoop`.
- Ahoy's automatic KCC runs in the schedule passed to `AhoyPlugins::new`.
  `AhoyPlugins::default()` uses `FixedPostUpdate`; the KCC set is ordered before
  `PhysicsSystems::First`.

Risk 1: `AccumulatedInput` clearing. Ahoy clears transient input once in
`RunFixedMainLoop::AfterFixedMainLoop`, outside the replayed `FixedMain` loop.
During rollback, Lightyear replays `FixedMain` directly. If we rely on Ahoy's
stock clear, transient values such as `last_movement`, `swim_up`, and
`crouched` can persist across multiple replayed ticks. `lightyear_ahoy` should
either own a fixed-tick clear/reset that runs inside the replayed fixed path, or
restore `AccumulatedInput` as rollback state before each tick.

Risk 2: timer advancement. Ahoy advances jump/tac/mantle/crane/climbdown
stopwatches in `PreUpdate`, not in the replayed fixed schedules. During
rollback, those timers will not naturally advance per replay tick unless the
adapter handles them. The safest initial design is to treat `AccumulatedInput`
and `CharacterControllerState` as rollback state and/or move/tick the relevant
timers in a fixed, replayed adapter system.

Risk 3: KCC ordering. Simply adding an ordering constraint to Ahoy's default KCC
may be enough for non-rollback play, but prediction/rollback wants exact control
over when one KCC step happens for one restored input tick. The safer pattern is
the `bevy_netahoy` one: put Ahoy's automatic KCC schedule in an inert/custom
schedule and have `lightyear_ahoy` call `CharacterControllerStepper::step_entity`
manually in a fixed schedule after BEI `Apply`.

Important clarification: this is not an argument against direct BEI integration.
`lightyear_inputs_bei` should still be the primary way inputs are replicated and
restored. The question is whether Ahoy's automatic `run_kcc` system should
consume those restored inputs, or whether `lightyear_ahoy` should consume them
with an adapter-owned stepping system. The automatic KCC path can be viable if
we also guarantee all of the following:

- BEI `Apply` has run for the restored/replayed tick before `run_kcc`;
- Ahoy transient input is cleared per replayed fixed tick, not only once in
  `RunFixedMainLoop::AfterFixedMainLoop`;
- Ahoy input timers advance consistently during rollback replays;
- only the intended simulated entities have active Ahoy KCC components;
- `Transform.translation` is mirrored to `Position` before Lightyear prediction
  history records the tick.

If those guarantees are clean to express as system ordering and small adapter
systems, automatic KCC is acceptable. If they require special cases, manual
`step_entity` is simpler and easier to audit.

Risk 4: look policy. Ahoy's stock camera path mutates the camera transform from
`RotateCamera` events, copies camera transform to `CharacterLook` in
`RunFixedMainLoop::BeforeFixedMainLoop`, and copies `CharacterLook` back to the
camera in `Update`. For netcode, the simulation should not depend on an
untracked presentation transform. The FPS demo should make look a deterministic
input value, preferably absolute yaw/pitch sampled for the tick, then write
`CharacterLook` from that value before the KCC step. Mouse deltas can still be
used locally, but they should be accumulated into an authoritative yaw/pitch for
the input tick.

Native input would reduce some of these risks, but not remove all of them. A
compact `AhoyUserCommand` makes per-tick movement, look, and buttons explicit,
which avoids BEI action-entity setup and makes `AccumulatedInput` reset/timer
ownership clearer. However the adapter would still need to manually step Ahoy's
KCC in the predicted fixed path, mirror `Transform` to `Position`, restore
`CharacterControllerState` and `AccumulatedInput` across rollback, and choose an
authoritative look policy. The native path is simpler to reason about; it is not
a replacement for the Ahoy rollback adapter.

`bevy_netahoy` stays small because it is intentionally game-specific and
Source-style: one command struct, one KCC stepping path, one snapshot format,
manual prediction history, and no reusable integration surface. Lightyear has
more code because it supports multiple input frontends, generic component
rollback, entity mapping, prespawns, host/dedicated modes, interpolation,
correction, and reusable transports. The lesson for `lightyear_ahoy` is to keep
the integration crate opinionated and narrow even though it sits on a more
general framework.

### What `bevy_netahoy` Does

`bevy_netahoy` uses a Source-style user command model. The example gathers raw
Bevy keyboard/mouse input into a `ClientInput` resource, builds an
`AhoyUserCmd { sequence, movement, look, buttons }`, predicts locally with
`NetAhoyStepper::pmove`, stores the command in local history, and sends a packet
containing recent commands to the server.

The server queues `AhoyUserCmdPacket`s per player, drops old/duplicate command
sequences, and runs each accepted command through the same
`NetAhoyStepper::pmove` path. Snapshots include the last processed sequence and
enough controller state for the client to compare, restore, and replay later
commands after correction.

The important part for this plan is that `bevy_netahoy` does not network BEI
action state for movement. It adds `EnhancedInputPlugin` in the example, but the
movement networking path is the compact user command path.

How it handles the integration risks:

- Manual KCC stepping: `AhoyPlugins::new(NetAhoyKccSchedule)` parks Ahoy's
  automatic KCC in a custom schedule. The example does not add
  `NetAhoyKccRunnerPlugin`, so that schedule is not run automatically. Client
  prediction, client replay, and server command consumption all call
  `NetAhoyStepper::pmove`, which calls `CharacterControllerStepper::step_entity`
  exactly once per consumed command and immediately mirrors `Transform` into
  `Position`.
- Accumulated input/timers: `pmove` owns the whole fixed-tick sequence. It ticks
  Ahoy input stopwatches by `Time<Fixed>::timestep()`, clears transient fields
  (`last_movement`, `swim_up`, `crouched`), applies the current command, and then
  steps the KCC. Prediction frames clone both `CharacterControllerState` and
  `AccumulatedInput`, and restore puts those back before replay. It does not rely
  on Ahoy's stock `RunFixedMainLoop::AfterFixedMainLoop` clear for rollback
  correctness.
- Look: local mouse deltas update a `ClientLook` resource in variable update.
  The fixed input gather copies absolute yaw/pitch into `AhoyUserCmd.look`.
  `pmove` writes those absolute values into `CharacterLook` before stepping. The
  camera follows the predicted KCC for presentation, but camera transform is not
  the movement simulation input.

## Avian Sync Mode Decision

Initial choice: `AvianReplicationMode::PositionButInterpolateTransform`.

Reasoning:

- We still replicate and predict `Position`, which is compact and matches
  Lightyear's Avian lag-compensation history.
- Ahoy writes `Transform`, so `lightyear_ahoy` will explicitly mirror the result
  into `Position` after each KCC step.
- Visual correction and frame interpolation should apply to `Transform`, because
  camera and presentation are transform-facing concerns in an FPS.
- This is closer to the current 2D FPS visual model than pure
  `AvianReplicationMode::Position`.

Fallback: use `AvianReplicationMode::Position` if `PositionButInterpolateTransform`
causes correction/camera ordering issues. In that fallback, the KCC adapter still
copies `Transform.translation` to `Position`, but presentation is driven from
`Position` syncing back to `Transform`.

Avoid `AvianReplicationMode::Transform` for the first pass. It is heavier to
replicate, less aligned with Avian lag-compensation history, and not needed if
we keep the explicit Ahoy `Transform -> Position` copy.

## Integration Crate Shape

Planned integration crate files:

- `crates/integration/ahoy/Cargo.toml`
- `crates/integration/ahoy/CHANGELOG.md`
- `crates/integration/ahoy/src/lib.rs`
- `crates/integration/ahoy/src/plugin.rs`
- `crates/integration/ahoy/src/stepper.rs`
- `crates/integration/ahoy/src/native.rs`

Package/library name: `lightyear_ahoy`.

The integration crate should be small and focused:

- provide `LightyearAhoyPlugin`;
- provide a manual KCC stepping system/system-param around Ahoy's
  `CharacterControllerStepper`;
- provide rollback/prediction registration helpers for Ahoy-owned state;
- provide compact serializable state wrappers only if directly replicating
  Ahoy's full components is not viable;
- define schedule sets so callers can order game systems relative to
  `Input -> Ahoy step -> Transform to Position copy -> PredictionHistory`;
- re-export commonly used types through `prelude`.

The integration crate should not define FPS weapons, maps, score, camera
presentation, or game-specific input bindings. Those stay in the demo.

Dependency note: `bevy_ahoy` should be pinned to a specific git revision unless
there is a compatible crates.io release with manual `step_entity` support.

## Demo Crate Shape

Planned files:

- `demos/fps/Cargo.toml`
- `demos/fps/README.md`
- `demos/fps/src/main.rs`
- `demos/fps/src/protocol.rs`
- `demos/fps/src/shared.rs`
- `demos/fps/src/weapons.rs`
- `demos/fps/src/client.rs`
- `demos/fps/src/server.rs`
- `demos/fps/src/renderer.rs`
- `demos/fps/src/automation.rs`

The demo should depend on `lightyear_ahoy` and should only contain demo-specific
gameplay, rendering, automation, and debug UI.

## Protocol Components

Player components:

- `PlayerId(PeerId)`
- `PlayerMarker`
- `Health`
- `Score`
- `WeaponState`
- `CharacterLook`
- `CharacterControllerState` or a compact `NetworkedAhoyState`
- `Position`
- `Rotation`
- `LinearVelocity`

Weapon/projectile components:

- `WeaponKind`
- `HitscanShotId`
- `ProjectileSpawn`
- `RocketMarker`
- `ProjectileOwner`
- `DespawnAfter`

Input actions:

- movement axis
- look delta or look absolute yaw/pitch
- jump
- crouch
- fire primary
- switch weapon

The input plugin should enable lag-compensation metadata so the server receives
each client's interpolation delay. Hitscan validation and projectile spawn
fairness both need that.

## Implementation Phases

### Phase 1: Integration Crate Skeleton

- [x] Add `crates/integration/ahoy/Cargo.toml`.
- [x] Add `lightyear_ahoy` to workspace members and workspace dependencies.
- [x] Add pinned `bevy_ahoy` dependency with `serialize`.
- [x] Add `LightyearAhoyPlugin` skeleton and prelude.
- [x] Ensure `cargo check -p lightyear_ahoy --all-features` loads the workspace.

### Phase 2: Integration Movement Adapter

- [x] Define the initial conservative adapter API without baking the FPS demo
      into the integration crate.
- [x] Add first-class BEI-facing integration for Ahoy movement actions by
      sampling BEI action values into hidden native `AhoyUserCommand` snapshots.
- [ ] Decide whether a second action-entity-networked BEI path is still useful
      for projects that need raw BEI state replicated.
- [x] Implement the conservative KCC stepping adapter:
  - [x] run after Lightyear native input restore/apply;
  - [x] write `AccumulatedInput` and `CharacterLook` from `AhoyUserCommand`;
  - [x] call `CharacterControllerStepper::step_entity`;
  - [x] copy `Transform.translation` to `Position`.
- [x] Add a native `AhoyUserCommand` fallback/reference path.
- [x] Register rollback/prediction for reusable Ahoy movement state needed
      to replay.
- [x] Define public schedule sets and ordering docs.
- [ ] Decide whether to support full `CharacterControllerState` replication or
      a compact `NetworkedAhoyState`.

### Phase 3: FPS Demo Skeleton And Movement

- [ ] Add `demos/fps/Cargo.toml`.
- [ ] Depend on `lightyear_ahoy`.
- [ ] Add module skeleton and README.
- [ ] Spawn server-authoritative player capsules on client connect.
- [ ] Add Ahoy controller/collider/collision layers.
- [ ] Implement FPS input mapping into the `lightyear_ahoy` adapter API.
- [ ] Verify local movement, jump, crouch, ramps, and correction smoothing.

### Phase 4: Remote Interpolation And Presentation

- [ ] Interpolate remote players.
- [ ] Hide or ghost confirmed local server entity on clients.
- [ ] Attach first-person camera to local predicted/presentation entity.
- [ ] Add simple third-person remote capsules.
- [ ] Add debug UI for local tick, interpolation tick, ping, weapon, and last
      correction mode.

### Phase 5: Hitscan

- [ ] Add hitscan input and cooldown.
- [ ] Client spawns immediate local tracer.
- [ ] Server validates with `LagCompensationSpatialQuery`.
- [ ] Server sends/replicates hit result.
- [ ] Render authoritative hit marker and optional rewind capsule debug.
- [ ] Test against moving remote targets under latency and packet loss.

### Phase 6: Slow Projectile

- [ ] Add rocket-like projectile spawn metadata.
- [ ] Shooter prespawns/predicts projectile immediately with stable hash.
- [ ] Server spawns authoritative projectile and matches shooter prespawn.
- [ ] Remote clients buffer projectile spawn until their interpolation timeline
      reaches the fire tick.
- [ ] Spawn remote projectile at time-corrected position:
      `origin + direction * speed * elapsed_since_fire`.
- [ ] Server owns projectile hit/explosion/despawn.
- [ ] Add visual correction for predicted shooter projectile if server spawn
      differs.

### Phase 7: Rocket Policy And Optional Impulses

- [ ] Decide whether first version includes rocket jump impulse.
- [ ] If included, apply impulse authoritatively and register enough rollback
      state to replay it.
- [ ] Document projectile lag-comp policy clearly:
  - hitscan rewinds targets exactly;
  - slow projectiles compensate spawn time and then simulate forward
    authoritatively.

### Phase 8: Automation And Verification

- [ ] Add headless scripted client movement.
- [ ] Add scripted hitscan test against moving target.
- [ ] Add scripted rocket spawn-timing test.
- [ ] Run under poor network conditions.
- [ ] Capture known-good debug traces or screenshots for README.

## Known Challenges

### Ahoy Dependency Stability

`bevy_netahoy` uses Ahoy's `step-entity` branch because manual per-entity KCC
stepping is central to prediction/replay. We should pin a specific revision.
Depending on a moving branch in the integration crate is likely to create random
breakage.
The dependency should be owned by `lightyear_ahoy`, so the demo and future users
consume one Lightyear integration dependency instead of each pinning Ahoy
themselves.

### Rollback State Completeness

Ahoy movement depends on more than `Position` and `LinearVelocity`. We need to
restore controller state and accumulated input/timers across rollback. If this
is incomplete, the symptom will be corrections after jump/crouch/mantle even
when position error looks small.

This is the main reason the adapter belongs in `crates/integration`: it is not
just example glue. It defines which Ahoy state Lightyear needs to consider part
of replayable movement.

### Schedule Ordering

The critical invariant is:

`Lightyear input for tick -> Ahoy step -> Transform to Position copy -> Avian/Lightyear prediction history`.

If prediction history records before the copy, rollback will compare stale
positions. If physics sync writes `Transform` back into `Position` at the wrong
time, it can erase the corrected state.

### Camera And Visual Correction

The camera should follow a smoothed/presentation transform, not necessarily the
raw corrected physics state. Otherwise small reconciliation corrections will be
felt as camera pops.

### Remote Player Collision

First pass should not make remote interpolated players solid for local
prediction. Solid remote players raise the question of which remote pose should
participate in rollback and can create visible shaking.

### Hitscan Time Semantics

Hitscan should validate against what the shooter saw. The server must use the
client's interpolation delay, and the history buffer must contain the target
poses surrounding that rewind tick.

### Projectile Time Semantics

Slow projectiles are not just delayed hitscan. For remote clients, spawning as
soon as the replication packet arrives makes the rocket appear too early or too
late relative to the interpolated shooter. The remote visual should be spawned
on the interpolation timeline, then fast-forwarded by the elapsed interpolated
time.

### Rocket Jumps

Rocket jumps couple projectile validation to player impulses. If the client
predicts the impulse, rollback must replay projectile explosion and movement
state consistently. This should be a second milestone after basic projectiles
are stable.

### Diagnostics

Without visible debug state, this demo will be hard to trust. It should show at
least correction distance, last acknowledged fire tick, interpolation tick, and
whether a hit was validated normally or with lag compensation.

## Open Decisions

- [x] Use `leafwing-input-manager`, native Lightyear input, or
      `bevy_enhanced_input` for this demo? Updated implementation state:
      the demo now uses BEI bindings via `AhoyBeiInputPlugin`, while
      `AhoyUserCommand` remains the hidden network/replay format.
- [ ] What should the generic `lightyear_ahoy` input adapter trait/API look
      like? It needs to support the FPS demo without hard-coding FPS actions.
- [x] Represent look as absolute yaw/pitch in input, or as per-frame mouse
      delta converted before fixed tick? Decision: the BEI bridge accumulates
      `RotateCamera` deltas into `AhoyBeiLook`, then writes absolute yaw/pitch
      into `AhoyUserCommand.look`.
- [ ] Replicate full `CharacterControllerState` or a compact wrapper?
- [ ] Include rocket jump in first public version?
- [x] Should `lightyear_ahoy` expose only a system-param/stepper, or also a
      higher-level plugin that wires prediction registration automatically?
      Initial implementation exposes both `LightyearAhoyStepper` and
      `LightyearAhoyPlugin`; native command stepping is in
      `NativeAhoyInputPlugin`.

## References

- Current 3D Avian character example: `examples/avian_3d_character`
- Current 2D FPS lag-compensation example: `examples/fps`
- Current projectile exploration example: `examples/projectiles`
- Existing Avian integration: `crates/integration/avian`
- Planned Ahoy integration: `crates/integration/ahoy`
- Inspiration: https://github.com/smoked-dev/bevy_netahoy
- Ahoy dependency used by netahoy: https://github.com/smoked-dev/bevy_ahoy
