use crate::HostClientMode;
use crate::automation::AutomationClientPlugin;
use crate::hit_detection::{HitImpact, HitPolicy, client_reported};
use crate::protocol::*;
use crate::representation::{RepresentationKind, fire_data_entity::FireData};
use crate::timeline::TimelinePolicy;
use crate::trajectory::{TrajectoryKind, linear};
use avian2d::prelude::*;
use bevy::ecs::relationship::Relationship;
use bevy::prelude::*;
use bevy_enhanced_input::EnhancedInputSystems;
use bevy_enhanced_input::action::TriggerState;
use bevy_enhanced_input::context::ExternallyMocked;
use bevy_enhanced_input::prelude::{
    ActionMock, ActionValue, ActionValueDim, Binding, Bindings, Cardinal, MockSpan,
};
use lightyear::input::bei::prelude::*;
use lightyear::input::client::InputSystems;
use lightyear::prelude::client::*;
use lightyear::prelude::*;

pub struct ExampleClientPlugin;

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(AutomationClientPlugin);
        app.init_resource::<client_reported::ReportedClientHits>();
        app.add_observer(add_previous_position_to_interpolated_projectile);
        app.add_systems(
            PreUpdate,
            strip_rigid_bodies_from_interpolated_entities.after(ReplicationSystems::Receive),
        );
        app.add_systems(
            FixedPreUpdate,
            (
                update_local_player_action_markers,
                update_global_action_markers,
            )
                .before(EnhancedInputSystems::Update)
                .before(InputSystems::BufferClientInputs),
        );
        app.add_systems(Update, clear_projectiles_when_axes_change);
        app.add_systems(
            FixedPostUpdate,
            (client_reported::hitscan_hits, client_reported::linear_hits)
                .after(PhysicsSystems::StepSimulation),
        );
    }
}

/// Interpolated entities are render-only timeline samples, not physics bodies.
///
/// `RigidBody` is replicated for predicted state projectiles, so it can also
/// arrive on interpolated players and projectiles. Letting Avian simulate those
/// delayed samples feeds interpolation-owned rotations into the solver and can
/// produce an invalid rotation during writeback. Remove the body immediately
/// after replication; Lightyear's Avian integration still copies the sampled
/// `Position` and `Rotation` into `Transform` for rendering.
fn strip_rigid_bodies_from_interpolated_entities(
    interpolated: Query<Entity, (With<Interpolated>, With<RigidBody>)>,
    mut commands: Commands,
) {
    for entity in &interpolated {
        commands.entity(entity).remove::<RigidBody>();
    }
}

/// Interpolated state projectiles receive sampled positions, but the previous
/// sweep endpoint is intentionally local state and is not replicated. Seed it
/// when the timeline entity becomes usable so client-reported linear hits can
/// sweep subsequent interpolation movement.
fn add_previous_position_to_interpolated_projectile(
    trigger: On<Add, (BulletMarker, Interpolated, Position)>,
    projectiles: Query<
        &Position,
        (
            With<BulletMarker>,
            With<Interpolated>,
            Without<linear::PreviousProjectilePosition>,
        ),
    >,
    mut commands: Commands,
) {
    if let Ok(position) = projectiles.get(trigger.entity) {
        commands
            .entity(trigger.entity)
            .insert(linear::PreviousProjectilePosition(position.0));
    }
}

/// A server axis change rebuilds all network entities. Clear immediate local
/// projectile copies too, so no visual from the previous configuration leaks
/// into the new arena. Players themselves are removed by replicated despawns.
fn clear_projectiles_when_axes_change(
    changed: Query<
        (),
        (
            With<ClientContext>,
            Or<(
                Changed<TrajectoryKind>,
                Changed<RepresentationKind>,
                Changed<HitPolicy>,
                Changed<TimelinePolicy>,
            )>,
        ),
    >,
    projectiles: Query<Entity, Or<(With<BulletMarker>, With<FireData>, With<HitImpact>)>>,
    mut reported_hits: ResMut<client_reported::ReportedClientHits>,
    mut commands: Commands,
) {
    if changed.is_empty() {
        return;
    }
    reported_hits.clear();
    for entity in &projectiles {
        commands.entity(entity).try_despawn();
    }
}

fn update_local_player_action_markers(
    client: Query<&LocalId, With<Client>>,
    players: Query<(&PlayerId, Has<Controlled>), With<PlayerMarker>>,
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

    for (entity, action_of, has_marker, externally_mocked, has_bindings) in &movement_actions {
        configure_player_action(
            &mut commands,
            entity,
            is_local_action(action_of, &players, client_id.0, host_client.is_some()),
            has_marker,
            externally_mocked,
            PlayerActionSource::Movement { has_bindings },
        );
    }
    for (entity, action_of, has_marker, externally_mocked, mock) in &cursor_actions {
        configure_player_action(
            &mut commands,
            entity,
            is_local_action(action_of, &players, client_id.0, host_client.is_some()),
            has_marker,
            externally_mocked,
            PlayerActionSource::Cursor { mock },
        );
    }
    for (entity, action_of, has_marker, externally_mocked, has_bindings) in &shoot_actions {
        configure_player_action(
            &mut commands,
            entity,
            is_local_action(action_of, &players, client_id.0, host_client.is_some()),
            has_marker,
            externally_mocked,
            PlayerActionSource::Shoot { has_bindings },
        );
    }
}

fn is_local_action(
    action_of: &ActionOf<PlayerContext>,
    players: &Query<(&PlayerId, Has<Controlled>), With<PlayerMarker>>,
    client_id: PeerId,
    host_client: bool,
) -> bool {
    players
        .get(action_of.get())
        .is_ok_and(|(player_id, controlled)| {
            player_id.0 == client_id && (controlled || host_client)
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
    active: bool,
    has_marker: bool,
    externally_mocked: bool,
    source: PlayerActionSource<'_>,
) {
    let mut action = commands.entity(entity);
    if active {
        if externally_mocked {
            action.try_remove::<ExternallyMocked>();
        }
        if !has_marker {
            action.insert(InputMarker::<PlayerContext>::default());
        }
        match source {
            PlayerActionSource::Movement { has_bindings } if !has_bindings => {
                action.insert(Bindings::spawn(Cardinal::wasd_keys()));
            }
            PlayerActionSource::Cursor { mock } if !mock.is_some_and(|mock| mock.enabled) => {
                let value = mock
                    .map(|mock| mock.value)
                    .map(|value| value.convert(ActionValueDim::Axis2D))
                    .unwrap_or_else(|| ActionValue::zero(ActionValueDim::Axis2D));
                action.insert(ActionMock::new(
                    TriggerState::Fired,
                    value,
                    MockSpan::Manual,
                ));
            }
            PlayerActionSource::Shoot { has_bindings } if !has_bindings => {
                action.insert(Bindings::spawn_one((
                    Binding::from(KeyCode::Space),
                    Name::from("Binding"),
                )));
            }
            _ => {}
        }
    } else {
        if has_marker {
            action.try_remove::<InputMarker<PlayerContext>>();
        }
        if !externally_mocked {
            action.insert(ExternallyMocked);
        }
    }
}

#[allow(clippy::type_complexity)]
fn update_global_action_markers(
    contexts: Query<(), With<ClientContext>>,
    trajectory: Query<
        (
            Entity,
            &ActionOf<ClientContext>,
            Has<InputMarker<ClientContext>>,
            Has<ExternallyMocked>,
            Has<Bindings>,
        ),
        With<Action<CycleTrajectory>>,
    >,
    representation: Query<
        (
            Entity,
            &ActionOf<ClientContext>,
            Has<InputMarker<ClientContext>>,
            Has<ExternallyMocked>,
            Has<Bindings>,
        ),
        With<Action<CycleRepresentation>>,
    >,
    hit_policy: Query<
        (
            Entity,
            &ActionOf<ClientContext>,
            Has<InputMarker<ClientContext>>,
            Has<ExternallyMocked>,
            Has<Bindings>,
        ),
        With<Action<CycleHitPolicy>>,
    >,
    timeline: Query<
        (
            Entity,
            &ActionOf<ClientContext>,
            Has<InputMarker<ClientContext>>,
            Has<ExternallyMocked>,
            Has<Bindings>,
        ),
        With<Action<CycleTimeline>>,
    >,
    mut commands: Commands,
) {
    for (entity, action_of, marker, mocked, bindings) in &trajectory {
        configure_global_action(
            &mut commands,
            &contexts,
            entity,
            action_of,
            marker,
            mocked,
            bindings,
            KeyCode::KeyQ,
        );
    }
    for (entity, action_of, marker, mocked, bindings) in &representation {
        configure_global_action(
            &mut commands,
            &contexts,
            entity,
            action_of,
            marker,
            mocked,
            bindings,
            KeyCode::KeyE,
        );
    }
    for (entity, action_of, marker, mocked, bindings) in &hit_policy {
        configure_global_action(
            &mut commands,
            &contexts,
            entity,
            action_of,
            marker,
            mocked,
            bindings,
            KeyCode::KeyR,
        );
    }
    for (entity, action_of, marker, mocked, bindings) in &timeline {
        configure_global_action(
            &mut commands,
            &contexts,
            entity,
            action_of,
            marker,
            mocked,
            bindings,
            KeyCode::KeyT,
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
    let mut action = commands.entity(entity);
    if externally_mocked {
        action.try_remove::<ExternallyMocked>();
    }
    if !has_marker {
        action.insert(InputMarker::<ClientContext>::default());
    }
    if !has_bindings {
        action.insert(Bindings::spawn_one((
            Binding::from(key),
            Name::from("Binding"),
        )));
    }
}
