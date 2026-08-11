use crate::automation::AutomationClientPlugin;
use crate::hit_detection::{HitImpact, HitPolicy, client_reported, hit_policy_is};
use crate::protocol::*;
use crate::representation::{RepresentationKind, fire_data_entity::FireData};
use crate::timeline::TimelinePolicy;
use crate::trajectory::{TrajectoryKind, linear};
use avian2d::prelude::*;
use bevy::ecs::relationship::Relationship;
use bevy::prelude::*;
use bevy_enhanced_input::action::TriggerState;
use bevy_enhanced_input::context::ExternallyMocked;
use bevy_enhanced_input::prelude::{
    ActionMock, ActionValue, ActionValueDim, Binding, Bindings, Cardinal, MockSpan,
};
use lightyear::input::bei::prelude::*;
use lightyear::interpolation::plugin::InterpolationSystems;
use lightyear::prelude::client::*;
use lightyear::prelude::*;

pub struct ExampleClientPlugin;

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(AutomationClientPlugin);
        app.init_resource::<client_reported::ReportedClientHits>();
        app.add_observer(add_rigid_body_to_predicted_simulation);
        app.add_observer(seed_interpolated_projectile_sweep_start);
        app.add_observer(configure_player_action_on_insert);
        app.add_observer(configure_controlled_player_actions);
        app.add_observer(configure_global_action_on_insert);
        app.add_systems(Update, clear_projectiles_on_reset);
        app.add_systems(
            FixedPostUpdate,
            (client_reported::hitscan_hits, client_reported::linear_hits)
                .run_if(hit_policy_is(HitPolicy::ClientReported))
                .after(PhysicsSystems::StepSimulation),
        );
        // State-entity projectiles on the interpolated timeline move in
        // `Update`, not in Avian's fixed physics schedule. Run the same sweep
        // after that sampling too. Each invocation advances
        // `ProjectileSweepStart`, so entities that did not move in that
        // schedule simply produce a zero-length segment.
        //
        // This cannot be one `AfterFixedMainLoop` system: that set runs before
        // `Update` samples interpolation, and it runs only once when a rendered
        // frame executes several catch-up fixed ticks. Predicted projectiles
        // must test every fixed-tick segment, while interpolated projectiles
        // must test the newly sampled render segment.
        app.add_systems(
            Update,
            client_reported::linear_hits
                .run_if(hit_policy_is(HitPolicy::ClientReported))
                .after(InterpolationSystems::Interpolate),
        );
    }
}

/// Add the locally derived physics role only to entities this client simulates.
///
/// `RigidBody` is deliberately not replicated. Replicating it to an
/// interpolated entity would make Avian eagerly insert default pose components
/// before Lightyear has sampled the real network pose, briefly rendering the
/// entity at the origin. Predicted players and linear state projectiles still
/// need a kinematic body so Avian integrates their replicated velocity.
fn add_rigid_body_to_predicted_simulation(
    trigger: On<Add, (Predicted, Position, LinearVelocity)>,
    simulated: Query<
        (),
        (
            With<Predicted>,
            With<Position>,
            With<LinearVelocity>,
            Without<RigidBody>,
            Or<(With<PlayerMarker>, With<BulletMarker>)>,
        ),
    >,
    mut commands: Commands,
) {
    if simulated.contains(trigger.entity) {
        commands.entity(trigger.entity).insert(RigidBody::Kinematic);
    }
}

/// Give an interpolated state projectile its first collision-sweep endpoint.
///
/// `ProjectileSweepStart` is local collision bookkeeping, not network state.
/// Seeding it from the first real sampled position makes the first segment
/// zero-length instead of incorrectly sweeping from the world origin.
fn seed_interpolated_projectile_sweep_start(
    trigger: On<Add, (BulletMarker, Interpolated, Position)>,
    projectiles: Query<
        &Position,
        (
            With<BulletMarker>,
            With<Interpolated>,
            Without<linear::ProjectileSweepStart>,
        ),
    >,
    mut commands: Commands,
) {
    if let Ok(position) = projectiles.get(trigger.entity) {
        commands
            .entity(trigger.entity)
            .insert(linear::ProjectileSweepStart(position.0));
    }
}

/// A server reset rebuilds all network entities. Clear immediate local
/// projectile copies too, so no visual from the previous configuration leaks
/// into the new arena. Players themselves are removed by replicated despawns.
fn clear_projectiles_on_reset(
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

/// Configure an action when its replicated relationship and typed action are
/// ready. This covers the ordering where the action arrives after its player.
fn configure_player_action_on_insert(
    trigger: On<
        Insert,
        (
            ActionOf<PlayerContext>,
            Action<MovePlayer>,
            Action<MoveCursor>,
            Action<Shoot>,
        ),
    >,
    action_of: Query<&ActionOf<PlayerContext>>,
    controlled_players: Query<(), (With<PlayerMarker>, With<Controlled>)>,
    movement_actions: Query<
        (
            Has<InputMarker<PlayerContext>>,
            Has<ExternallyMocked>,
            Has<Bindings>,
        ),
        With<Action<MovePlayer>>,
    >,
    cursor_actions: Query<
        (
            Has<InputMarker<PlayerContext>>,
            Has<ExternallyMocked>,
            Option<&ActionMock>,
        ),
        With<Action<MoveCursor>>,
    >,
    shoot_actions: Query<
        (
            Has<InputMarker<PlayerContext>>,
            Has<ExternallyMocked>,
            Has<Bindings>,
        ),
        With<Action<Shoot>>,
    >,
    mut commands: Commands,
) {
    let Ok(action_of) = action_of.get(trigger.entity) else {
        return;
    };
    configure_player_action_entity(
        &mut commands,
        trigger.entity,
        controlled_players.contains(action_of.get()),
        &movement_actions,
        &cursor_actions,
        &shoot_actions,
    );
}

/// Configure existing actions when `Controlled` arrives after them.
fn configure_controlled_player_actions(
    trigger: On<Add, (PlayerMarker, Controlled, Actions<PlayerContext>)>,
    players: Query<&Actions<PlayerContext>, (With<PlayerMarker>, With<Controlled>)>,
    movement_actions: Query<
        (
            Has<InputMarker<PlayerContext>>,
            Has<ExternallyMocked>,
            Has<Bindings>,
        ),
        With<Action<MovePlayer>>,
    >,
    cursor_actions: Query<
        (
            Has<InputMarker<PlayerContext>>,
            Has<ExternallyMocked>,
            Option<&ActionMock>,
        ),
        With<Action<MoveCursor>>,
    >,
    shoot_actions: Query<
        (
            Has<InputMarker<PlayerContext>>,
            Has<ExternallyMocked>,
            Has<Bindings>,
        ),
        With<Action<Shoot>>,
    >,
    mut commands: Commands,
) {
    let Ok(actions) = players.get(trigger.entity) else {
        return;
    };
    for action in actions.iter() {
        configure_player_action_entity(
            &mut commands,
            action,
            true,
            &movement_actions,
            &cursor_actions,
            &shoot_actions,
        );
    }
}

enum PlayerActionSource<'a> {
    Movement { has_bindings: bool },
    Cursor { mock: Option<&'a ActionMock> },
    Shoot { has_bindings: bool },
}

/// Apply the correct local-only setup for whichever typed player action lives
/// on `entity`. Both sides of the replication race use this helper.
fn configure_player_action_entity(
    commands: &mut Commands,
    entity: Entity,
    active: bool,
    movement_actions: &Query<
        (
            Has<InputMarker<PlayerContext>>,
            Has<ExternallyMocked>,
            Has<Bindings>,
        ),
        With<Action<MovePlayer>>,
    >,
    cursor_actions: &Query<
        (
            Has<InputMarker<PlayerContext>>,
            Has<ExternallyMocked>,
            Option<&ActionMock>,
        ),
        With<Action<MoveCursor>>,
    >,
    shoot_actions: &Query<
        (
            Has<InputMarker<PlayerContext>>,
            Has<ExternallyMocked>,
            Has<Bindings>,
        ),
        With<Action<Shoot>>,
    >,
) {
    if let Ok((has_marker, externally_mocked, has_bindings)) = movement_actions.get(entity) {
        configure_player_action(
            commands,
            entity,
            active,
            has_marker,
            externally_mocked,
            PlayerActionSource::Movement { has_bindings },
        );
    } else if let Ok((has_marker, externally_mocked, mock)) = cursor_actions.get(entity) {
        configure_player_action(
            commands,
            entity,
            active,
            has_marker,
            externally_mocked,
            PlayerActionSource::Cursor { mock },
        );
    } else if let Ok((has_marker, externally_mocked, has_bindings)) = shoot_actions.get(entity) {
        configure_player_action(
            commands,
            entity,
            active,
            has_marker,
            externally_mocked,
            PlayerActionSource::Shoot { has_bindings },
        );
    }
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
fn configure_global_action_on_insert(
    trigger: On<
        Insert,
        (
            ClientContext,
            ActionOf<ClientContext>,
            Action<CycleTrajectory>,
            Action<CycleRepresentation>,
            Action<CycleHitPolicy>,
            Action<CycleTimeline>,
        ),
    >,
    contexts: Query<&Actions<ClientContext>, With<ClientContext>>,
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
    // The context and its action entities are replicated independently. One
    // observer handles either arrival order: configure the triggering action,
    // and when the context itself triggers, revisit its existing relationship.
    if let Ok(actions) = contexts.get(trigger.entity) {
        for action in actions.iter() {
            configure_global_action_entity(
                &mut commands,
                &contexts,
                action,
                &trajectory,
                &representation,
                &hit_policy,
                &timeline,
            );
        }
    }
    configure_global_action_entity(
        &mut commands,
        &contexts,
        trigger.entity,
        &trajectory,
        &representation,
        &hit_policy,
        &timeline,
    );
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn configure_global_action_entity(
    commands: &mut Commands,
    contexts: &Query<&Actions<ClientContext>, With<ClientContext>>,
    entity: Entity,
    trajectory: &Query<
        (
            Entity,
            &ActionOf<ClientContext>,
            Has<InputMarker<ClientContext>>,
            Has<ExternallyMocked>,
            Has<Bindings>,
        ),
        With<Action<CycleTrajectory>>,
    >,
    representation: &Query<
        (
            Entity,
            &ActionOf<ClientContext>,
            Has<InputMarker<ClientContext>>,
            Has<ExternallyMocked>,
            Has<Bindings>,
        ),
        With<Action<CycleRepresentation>>,
    >,
    hit_policy: &Query<
        (
            Entity,
            &ActionOf<ClientContext>,
            Has<InputMarker<ClientContext>>,
            Has<ExternallyMocked>,
            Has<Bindings>,
        ),
        With<Action<CycleHitPolicy>>,
    >,
    timeline: &Query<
        (
            Entity,
            &ActionOf<ClientContext>,
            Has<InputMarker<ClientContext>>,
            Has<ExternallyMocked>,
            Has<Bindings>,
        ),
        With<Action<CycleTimeline>>,
    >,
) {
    let action = trajectory
        .get(entity)
        .ok()
        .map(|action| (action, KeyCode::KeyQ))
        .or_else(|| {
            representation
                .get(entity)
                .ok()
                .map(|action| (action, KeyCode::KeyE))
        })
        .or_else(|| {
            hit_policy
                .get(entity)
                .ok()
                .map(|action| (action, KeyCode::KeyR))
        })
        .or_else(|| {
            timeline
                .get(entity)
                .ok()
                .map(|action| (action, KeyCode::KeyT))
        });
    let Some(((entity, action_of, marker, mocked, bindings), key)) = action else {
        return;
    };
    configure_global_action(
        commands, contexts, entity, action_of, marker, mocked, bindings, key,
    );
}

fn configure_global_action(
    commands: &mut Commands,
    contexts: &Query<&Actions<ClientContext>, With<ClientContext>>,
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
