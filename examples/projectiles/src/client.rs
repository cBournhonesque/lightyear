use crate::HostClientMode;
use crate::automation::AutomationClientPlugin;
use crate::protocol::*;
use crate::shared;
use crate::shared::color_from_id;
use avian2d::prelude::*;
use bevy::ecs::relationship::Relationship;
use bevy::prelude::*;
use bevy_enhanced_input::EnhancedInputSystems;
use bevy_enhanced_input::action::TriggerState;
use bevy_enhanced_input::bindings;
use bevy_enhanced_input::context::ExternallyMocked;
use bevy_enhanced_input::prelude::{
    ActionMock, ActionValue, ActionValueDim, Binding, Bindings, Cardinal, MockSpan,
};
use lightyear::input::bei::prelude::*;
use lightyear::input::client::InputSystems;
use lightyear::prediction::rollback::DisableRollback;
use lightyear::prelude::client::*;
use lightyear::prelude::*;

pub struct ExampleClientPlugin;

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(AutomationClientPlugin);
        app.add_observer(handle_predicted_spawn);
        app.add_observer(handle_interpolated_spawn);
        app.add_observer(handle_deterministic_spawn);
        app.add_observer(add_global_actions);
        app.add_systems(
            FixedPreUpdate,
            (
                update_active_player_action_markers,
                update_global_action_markers,
            )
                .before(EnhancedInputSystems::Update)
                .before(InputSystems::BufferClientInputs),
        );
    }
}

// When the predicted copy of the client-owned entity is spawned, do stuff
// - assign it a different saturation
// - add physics components so that its movement can be predicted
pub(crate) fn handle_predicted_spawn(
    trigger: On<Add, (PlayerMarker, Predicted)>,
    client: Single<&LocalId, With<Client>>,
    host_client: Option<Res<HostClientMode>>,
    mut commands: Commands,
    mut player_query: Query<(&PlayerId, &Position, &GameReplicationMode), With<Predicted>>,
) {
    let client_id = client.into_inner().0;
    if let Ok((player_id, pos, mode)) = player_query.get_mut(trigger.entity) {
        if mode == &GameReplicationMode::AllInterpolated {
            return;
        };
        match mode {
            GameReplicationMode::ClientSideHitDetection
            | GameReplicationMode::OnlyInputsReplicated => {
                // add these so we can do hit-detection on the client
                commands.entity(trigger.entity).insert((
                    Collider::rectangle(PLAYER_SIZE, PLAYER_SIZE),
                    RigidBody::Kinematic,
                ));
            }
            _ => {}
        };
        if player_id.0 != client_id {
            return;
        }
        // add actions on the local entity (remote predicted entities will have actions propagated by the server)
        if should_spawn_client_actions(&host_client) {
            info!(
                ?pos,
                "Adding actions to predicted player {:?}", trigger.entity
            );
            shared::spawn_player_actions(&mut commands, trigger.entity, player_id.0, *mode, false);
        }
    }
}

pub(crate) fn handle_interpolated_spawn(
    trigger: On<Add, (PlayerMarker, Interpolated)>,
    client: Single<&LocalId, With<Client>>,
    host_client: Option<Res<HostClientMode>>,
    mut interpolated: Query<
        (&PlayerId, &Interpolated, &GameReplicationMode),
        (With<Interpolated>, With<PlayerMarker>),
    >,
    mut commands: Commands,
) {
    let client_id = client.into_inner();
    if let Ok((player_id, interpolated, mode)) = interpolated.get_mut(trigger.entity) {
        if mode == &GameReplicationMode::ClientSideHitDetection {
            // add these so we can do hit-detection on the client
            commands
                .entity(trigger.entity)
                .insert((Collider::rectangle(PLAYER_SIZE, PLAYER_SIZE),));
        }
        // In the interpolated case, the client sends inputs but doesn't apply them.
        // Only the server applies the inputs, and the position changes are replicated back
        if let GameReplicationMode::AllInterpolated = mode
            && client_id.0 == player_id.0
            && should_spawn_client_actions(&host_client)
        {
            shared::spawn_player_actions(&mut commands, trigger.entity, player_id.0, *mode, false);
        }
    }
}

pub(crate) fn handle_deterministic_spawn(
    trigger: On<Add, PlayerMarker>,
    query: Query<(&PlayerId, &GameReplicationMode)>,
    client: Single<&LocalId, With<Client>>,
    host_client: Option<Res<HostClientMode>>,
    mut commands: Commands,
) {
    let client_id = client.into_inner();
    if let Ok((player_id, mode)) = query.get(trigger.entity)
        && mode == &GameReplicationMode::OnlyInputsReplicated
    {
        commands.entity(trigger.entity).insert((
            shared::player_bundle(player_id.0, GameReplicationMode::OnlyInputsReplicated),
            DeterministicPredicted {
                // make sure that we don't despawn the player if there is a rollback
                skip_despawn: true,
                ..default()
            },
        ));
        info!("Adding PlayerContext for player {:?}", player_id);

        // add actions for the local client
        if player_id.0 == client_id.0 && should_spawn_client_actions(&host_client) {
            info!(
                "Spawning actions for DeterministicPredicted player {:?}",
                player_id
            );
            shared::spawn_player_actions(&mut commands, trigger.entity, player_id.0, *mode, false);
        }
    }
}

pub(crate) fn add_global_actions(
    trigger: On<Add, ClientContext>,
    host_client: Option<Res<HostClientMode>>,
    mut commands: Commands,
) {
    if should_spawn_client_actions(&host_client) {
        shared::spawn_global_actions(&mut commands, trigger.entity, false);
    }
}

fn should_spawn_client_actions(host_client: &Option<Res<HostClientMode>>) -> bool {
    host_client.is_none()
}

fn update_active_player_action_markers(
    client: Query<&LocalId, With<Client>>,
    global_mode: Query<&GameReplicationMode, With<ClientContext>>,
    players: Query<(&PlayerId, &GameReplicationMode, Has<Controlled>), With<PlayerMarker>>,
    host_client: Option<Res<HostClientMode>>,
    movement_actions: Query<
        (
            Entity,
            &ActionOf<PlayerContext>,
            Has<InputMarker<PlayerContext>>,
            Has<ExternallyMocked>,
            Has<Bindings>,
        ),
        With<Action<MovePlayer>>,
    >,
    cursor_actions: Query<
        (
            Entity,
            &ActionOf<PlayerContext>,
            Has<InputMarker<PlayerContext>>,
            Has<ExternallyMocked>,
            Option<&ActionMock>,
        ),
        With<Action<MoveCursor>>,
    >,
    shoot_actions: Query<
        (
            Entity,
            &ActionOf<PlayerContext>,
            Has<InputMarker<PlayerContext>>,
            Has<ExternallyMocked>,
            Has<Bindings>,
        ),
        With<Action<Shoot>>,
    >,
    mut commands: Commands,
) {
    let Ok(client_id) = client.single() else {
        return;
    };
    let Ok(global_mode) = global_mode.single() else {
        return;
    };
    for (entity, action_of, has_marker, externally_mocked, has_bindings) in &movement_actions {
        configure_player_action(
            &mut commands,
            entity,
            is_active_local_action(
                action_of,
                &players,
                client_id.0,
                global_mode,
                host_client.is_some(),
            ),
            has_marker,
            externally_mocked,
            PlayerActionSource::Movement { has_bindings },
        );
    }
    for (entity, action_of, has_marker, externally_mocked, mock) in &cursor_actions {
        configure_player_action(
            &mut commands,
            entity,
            is_active_local_action(
                action_of,
                &players,
                client_id.0,
                global_mode,
                host_client.is_some(),
            ),
            has_marker,
            externally_mocked,
            PlayerActionSource::Cursor { mock },
        );
    }
    for (entity, action_of, has_marker, externally_mocked, has_bindings) in &shoot_actions {
        configure_player_action(
            &mut commands,
            entity,
            is_active_local_action(
                action_of,
                &players,
                client_id.0,
                global_mode,
                host_client.is_some(),
            ),
            has_marker,
            externally_mocked,
            PlayerActionSource::Shoot { has_bindings },
        );
    }
}

fn is_active_local_action(
    action_of: &ActionOf<PlayerContext>,
    players: &Query<(&PlayerId, &GameReplicationMode, Has<Controlled>), With<PlayerMarker>>,
    client_id: PeerId,
    global_mode: &GameReplicationMode,
    host_client: bool,
) -> bool {
    players
        .get(action_of.get())
        .is_ok_and(|(player_id, mode, controlled)| {
            player_id.0 == client_id && mode == global_mode && (controlled || host_client)
        })
}

enum PlayerActionSource<'a> {
    Movement { has_bindings: bool },
    Cursor { mock: Option<&'a ActionMock> },
    Shoot { has_bindings: bool },
}

fn configure_player_action(
    commands: &mut Commands,
    entity: Entity,
    is_active_local: bool,
    has_marker: bool,
    externally_mocked: bool,
    source: PlayerActionSource<'_>,
) {
    let mut entity_commands = commands.entity(entity);
    if is_active_local {
        if externally_mocked {
            entity_commands.try_remove::<ExternallyMocked>();
        }
        if !has_marker {
            entity_commands.insert(InputMarker::<PlayerContext>::default());
        }
        match source {
            PlayerActionSource::Movement { has_bindings } => {
                if !has_bindings {
                    entity_commands.insert(Bindings::spawn(Cardinal::wasd_keys()));
                }
            }
            PlayerActionSource::Cursor { mock } => {
                if !mock.is_some_and(|mock| mock.enabled) {
                    let value = mock
                        .map(|mock| mock.value)
                        .map(|value| value.convert(ActionValueDim::Axis2D))
                        .unwrap_or_else(|| ActionValue::zero(ActionValueDim::Axis2D));
                    entity_commands.insert(ActionMock::new(
                        TriggerState::Fired,
                        value,
                        MockSpan::Manual,
                    ));
                }
            }
            PlayerActionSource::Shoot { has_bindings } => {
                if !has_bindings {
                    entity_commands.insert(Bindings::spawn_one((
                        Binding::from(KeyCode::Space),
                        Name::from("Binding"),
                    )));
                }
            }
        }
    } else {
        if has_marker {
            entity_commands.try_remove::<InputMarker<PlayerContext>>();
        }
        if !externally_mocked {
            entity_commands.insert(ExternallyMocked);
        }
    }
}

fn update_global_action_markers(
    contexts: Query<(), With<ClientContext>>,
    projectile_actions: Query<
        (
            Entity,
            &ActionOf<ClientContext>,
            Has<InputMarker<ClientContext>>,
            Has<ExternallyMocked>,
            Has<Bindings>,
        ),
        With<Action<CycleProjectileMode>>,
    >,
    replication_actions: Query<
        (
            Entity,
            &ActionOf<ClientContext>,
            Has<InputMarker<ClientContext>>,
            Has<ExternallyMocked>,
            Has<Bindings>,
        ),
        With<Action<CycleReplicationMode>>,
    >,
    weapon_actions: Query<
        (
            Entity,
            &ActionOf<ClientContext>,
            Has<InputMarker<ClientContext>>,
            Has<ExternallyMocked>,
            Has<Bindings>,
        ),
        With<Action<CycleWeapon>>,
    >,
    mut commands: Commands,
) {
    for (entity, action_of, has_marker, externally_mocked, has_bindings) in &projectile_actions {
        configure_global_action(
            &mut commands,
            &contexts,
            entity,
            action_of,
            has_marker,
            externally_mocked,
            has_bindings,
            KeyCode::KeyE,
        );
    }
    for (entity, action_of, has_marker, externally_mocked, has_bindings) in &replication_actions {
        configure_global_action(
            &mut commands,
            &contexts,
            entity,
            action_of,
            has_marker,
            externally_mocked,
            has_bindings,
            KeyCode::KeyR,
        );
    }
    for (entity, action_of, has_marker, externally_mocked, has_bindings) in &weapon_actions {
        configure_global_action(
            &mut commands,
            &contexts,
            entity,
            action_of,
            has_marker,
            externally_mocked,
            has_bindings,
            KeyCode::KeyQ,
        );
    }
}

fn configure_global_action(
    commands: &mut Commands,
    contexts: &Query<(), With<ClientContext>>,
    entity: Entity,
    action_of: &ActionOf<ClientContext>,
    has_marker: bool,
    externally_mocked: bool,
    has_bindings: bool,
    key: KeyCode,
) {
    if !contexts.contains(action_of.get()) {
        return;
    }
    let mut entity_commands = commands.entity(entity);
    if externally_mocked {
        entity_commands.try_remove::<ExternallyMocked>();
    }
    if !has_marker {
        entity_commands.insert(InputMarker::<ClientContext>::default());
    }
    if !has_bindings {
        entity_commands.insert(Bindings::spawn_one((
            Binding::from(key),
            Name::from("Binding"),
        )));
    }
}
