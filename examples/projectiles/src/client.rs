use crate::protocol::*;
use crate::shared;
use crate::shared::{Rooms, color_from_id};
use avian2d::prelude::{Collider, RigidBody};
use bevy::prelude::*;
use bevy_enhanced_input::action::ActionMock;
use bevy_enhanced_input::bindings;
use core::time::Duration;
use lightyear::input::bei::prelude::*;
use lightyear::input::client::InputSet;
use lightyear::prediction::rollback::DisableRollback;
use lightyear::prelude::client::*;
use lightyear::prelude::*;

pub struct ExampleClientPlugin;

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(handle_predicted_spawn);
        app.add_observer(handle_interpolated_spawn);
        app.add_observer(handle_deterministic_spawn);
        app.add_observer(add_global_actions);
        // app.add_observer(cycle_projectile_mode);
        // app.add_observer(cycle_replication_mode);
        // app.add_systems(RunFixedMainLoop, cycle_replication_mode.in_set(RunFixedMainLoopSystem::BeforeFixedMainLoop), "gui");
        // app.add_systems(FixedUpdate, cycle_replication_mode_fixed_update);
    }
}

// When the predicted copy of the client-owned entity is spawned, do stuff
// - assign it a different saturation
// - add physics components so that its movement can be predicted
pub(crate) fn handle_predicted_spawn(
    trigger: Trigger<OnAdd, (PlayerId, Predicted)>,
    client: Single<&LocalId, With<Client>>,
    mode: Single<&GameReplicationMode, With<ClientContext>>,
    mut commands: Commands,
    mut player_query: Query<(&mut ColorComponent, &PlayerId), (With<Predicted>, With<Controlled>)>,
) {
    let client_id = client.into_inner().0;
    let replication_mode = mode.into_inner();
    if let Ok((mut color, player_id)) = player_query.get_mut(trigger.target()) {
        info!("Adding actions to predicted player {:?}", trigger.target());
        let hsva = Hsva {
            saturation: 0.4,
            ..Hsva::from(color.0)
        };
        color.0 = Color::from(hsva);
        if replication_mode == &GameReplicationMode::AllInterpolated {
            return;
        };
        match replication_mode {
            GameReplicationMode::ClientSideHitDetection
            | GameReplicationMode::OnlyInputsReplicated => {
                // add these so we can do hit-detection on the client
                commands.entity(trigger.target()).insert((
                    Collider::rectangle(PLAYER_SIZE, PLAYER_SIZE),
                    RigidBody::Kinematic,
                ));
            }
            _ => {}
        };
        // if player_id.0 != client_id {
        //     return;
        // }
        // add actions on the predicted entity
        add_actions(&mut commands, trigger.target());
    }
}

pub(crate) fn handle_interpolated_spawn(
    trigger: Trigger<OnAdd, (PlayerId, Interpolated)>,
    mode: Single<&GameReplicationMode, With<ClientContext>>,
    mut interpolated: Query<
        (&mut ColorComponent, &Interpolated),
        (With<Interpolated>, With<Controlled>),
    >,
    mut commands: Commands,
) {
    let replication_mode = mode.into_inner();
    if let Ok((mut color, interpolated)) = interpolated.get_mut(trigger.target()) {
        let hsva = Hsva {
            saturation: 0.1,
            ..Hsva::from(color.0)
        };
        color.0 = Color::from(hsva);
        // In the interpolated case, the client controls the confirmed entity
        if let GameReplicationMode::AllInterpolated = replication_mode {
            add_actions(&mut commands, interpolated.confirmed_entity);
        }
    }
}

pub(crate) fn handle_deterministic_spawn(
    trigger: Trigger<OnAdd, PlayerId>,
    client: Single<&LocalId, With<Client>>,
    mode: Single<&GameReplicationMode, With<ClientContext>>,
    mut commands: Commands,
) {
    let replication_mode = mode.into_inner();
    // TODO: it's controllable by the client, but it doesn't render!
    let client_id = client.into_inner();
    match replication_mode {
        GameReplicationMode::OnlyInputsReplicated => {
            commands
                .entity(trigger.target())
                .insert((shared::player_bundle(client_id.0), DeterministicPredicted));
            add_actions(&mut commands, trigger.target());
        }
        _ => {
            // handled in the predicted/interpolated spawn handlers
        }
    }
}

fn add_actions(commands: &mut Commands, player: Entity) {
    commands.entity(player).insert(PlayerContext);
    commands.spawn((
        ActionOf::<PlayerContext>::new(player),
        Action::<MovePlayer>::new(),
        Bindings::spawn(Cardinal::wasd_keys()),
    ));
    commands.spawn((
        ActionOf::<PlayerContext>::new(player),
        Action::<MoveCursor>::new(),
        // we use a mock to manually set the ActionState and ActionValue from the mouse position
        ActionMock::new(
            ActionState::Fired,
            ActionValue::zero(ActionValueDim::Axis2D),
            MockSpan::Manual,
        ),
    ));
    commands.spawn((
        ActionOf::<PlayerContext>::new(player),
        Action::<Shoot>::new(),
        Bindings::spawn_one((Binding::from(KeyCode::Space), Name::from("Binding"))),
    ));
    commands.spawn((
        ActionOf::<PlayerContext>::new(player),
        Action::<CycleWeapon>::new(),
        Bindings::spawn_one((Binding::from(KeyCode::KeyQ), Name::from("Binding"))),
    ));
}

pub(crate) fn add_global_actions(trigger: Trigger<OnAdd, ClientContext>, mut commands: Commands) {
    commands.spawn((
        ActionOf::<ClientContext>::new(trigger.target()),
        Action::<CycleProjectileMode>::new(),
        bindings![KeyCode::KeyE,],
    ));
    commands.spawn((
        ActionOf::<ClientContext>::new(trigger.target()),
        Action::<CycleReplicationMode>::new(),
        bindings![KeyCode::KeyR,],
    ));
}

pub fn cycle_replication_mode(
    timeline: Single<(&LocalTimeline, Has<Rollback>)>,
    action: Single<(Entity, &ActionValue, &ActionEvents), With<Action<CycleReplicationMode>>>,
) {
    let (timeline, rollback) = timeline.into_inner();
    let tick = timeline.tick();
    let (entity, action_value, action_events) = action.into_inner();
    trace!(
        ?tick,
        ?rollback,
        ?entity,
        "CycleReplicationMode PreUpdate action value: {:?}, events: {:?}",
        action_value,
        action_events
    );
}

pub fn cycle_replication_mode_fixed_update(
    timeline: Single<(&LocalTimeline, Has<Rollback>)>,
    action: Single<(Entity, &ActionValue, &ActionEvents), With<Action<CycleReplicationMode>>>,
) {
    let (timeline, rollback) = timeline.into_inner();
    let tick = timeline.tick();
    let (entity, action_value, action_events) = action.into_inner();
    trace!(
        ?tick,
        ?rollback,
        ?entity,
        "CycleReplicationMode FixedUpdate action value: {:?}, events: {:?}",
        action_value,
        action_events
    );
}
