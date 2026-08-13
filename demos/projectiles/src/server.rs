use crate::automation::{
    AutomationServerPlugin, initial_hit_policy, initial_representation, initial_timeline,
    initial_trajectory,
};
use crate::bot::BotClient;
#[cfg(feature = "client")]
use crate::bot::BotPlugin;
use crate::hit_detection::{
    HitImpact, HitPolicy, accept_hit, hit_policy_is, server_current,
    server_rewound::{self, LagCompensatedSilhouette},
};
use crate::protocol::*;
use crate::representation::{RepresentationKind, fire_data_entity::FireData};
use crate::shared;
use crate::timeline::TimelinePolicy;
use crate::trajectory::TrajectoryKind;
use avian2d::prelude::*;
use bevy::prelude::*;
use bevy_enhanced_input::EnhancedInputSystems;
use bevy_enhanced_input::prelude::*;
use lightyear::connection::client_of::ClientOf;
use lightyear::input::server::{InputSystems as ServerInputSystems, ServerInputConfig};
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use lightyear_avian2d::prelude::{
    LagCompensationHistory, LagCompensationPlugin, LagCompensationSystems,
};
use lightyear_examples_common::shared::SEND_INTERVAL;

pub struct ExampleServerPlugin;

impl Plugin for ExampleServerPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(AutomationServerPlugin);
        app.insert_resource(ReplicationMetadata::new(SEND_INTERVAL));
        app.add_plugins(LagCompensationPlugin);

        app.add_observer(handle_new_client);
        app.add_observer(spawn_player);
        app.add_observer(
            handle_client_reported_hit.run_if(hit_policy_is(HitPolicy::ClientReported)),
        );

        app.add_systems(
            Startup,
            (spawn_global_control, apply_initial_input_config).chain(),
        );
        app.add_systems(
            FixedPreUpdate,
            apply_global_actions_and_reset_arena.after(ServerInputSystems::UpdateActionState),
        );

        // Current-state collision still runs after physics so linear shots can
        // sweep the segment Avian actually moved this tick.
        app.add_systems(
            FixedPostUpdate,
            (server_current::hitscan_hits, server_current::linear_hits)
                .run_if(hit_policy_is(HitPolicy::ServerCurrent))
                .after(PhysicsSystems::StepSimulation),
        );
        // Rewound queries additionally run in Lightyear's documented lag-comp
        // collision set, after history and Avian's spatial query are current.
        app.add_systems(
            FixedPostUpdate,
            (server_rewound::hitscan_hits, server_rewound::linear_hits)
                .run_if(hit_policy_is(HitPolicy::ServerRewound))
                .in_set(LagCompensationSystems::Collisions)
                .after(PhysicsSystems::StepSimulation),
        );

        // A headless server has no real input device. BEI's preparation system
        // is unnecessary and can otherwise expect GUI resources.
        app.configure_sets(PreUpdate, EnhancedInputSystems::Prepare.run_if(|| false));

        #[cfg(feature = "client")]
        app.add_plugins(BotPlugin);
    }
}

pub(crate) fn handle_new_client(trigger: On<Add, LinkOf>, mut commands: Commands) {
    commands
        .entity(trigger.entity)
        .insert((ReplicationSender, Name::from("ClientOf")));
}

/// The global context has one action entity per independently selectable axis.
fn spawn_global_actions(commands: &mut Commands, context: Entity) {
    commands.spawn((
        ActionOf::<ClientContext>::new(context),
        Action::<CycleTrajectory>::new(),
        ReplicateLike { root: context },
    ));
    commands.spawn((
        ActionOf::<ClientContext>::new(context),
        Action::<CycleRepresentation>::new(),
        ReplicateLike { root: context },
    ));
    commands.spawn((
        ActionOf::<ClientContext>::new(context),
        Action::<CycleHitPolicy>::new(),
        ReplicateLike { root: context },
    ));
    commands.spawn((
        ActionOf::<ClientContext>::new(context),
        Action::<CycleTimeline>::new(),
        ReplicateLike { root: context },
    ));
}

/// Spawn the server-owned BEI action entities for one player.
///
/// `ActionOf` has a linked-spawn relationship, so despawning the player during
/// an arena restart also cleans up these action entities.
fn spawn_player_actions(commands: &mut Commands, player: Entity) {
    commands.spawn((
        ActionOf::<PlayerContext>::new(player),
        Action::<MovePlayer>::new(),
        ReplicateLike { root: player },
    ));
    commands.spawn((
        ActionOf::<PlayerContext>::new(player),
        Action::<MoveCursor>::new(),
        ReplicateLike { root: player },
    ));
    commands.spawn((
        ActionOf::<PlayerContext>::new(player),
        Action::<Shoot>::new(),
        ReplicateLike { root: player },
    ));
}

/// Spawn the one server-owned entity that stores the four current axis values.
fn spawn_global_control(mut commands: Commands) {
    let trajectory = initial_trajectory();
    let representation = initial_representation();
    let hit_policy = initial_hit_policy();
    let timeline = initial_timeline();
    info!(
        trajectory = trajectory.name(),
        representation = representation.name(),
        hit_policy = hit_policy.name(),
        timeline = timeline.name(),
        "Starting projectiles demo"
    );

    let context = commands
        .spawn((
            ClientContext,
            Replicate::to_clients(NetworkTarget::All),
            trajectory,
            representation,
            hit_policy,
            timeline,
            Name::new("ProjectileDemoConfig"),
        ))
        .id();
    spawn_global_actions(&mut commands, context);
}

fn apply_initial_input_config(
    timeline: Single<&TimelinePolicy, With<ClientContext>>,
    mut input_config: ResMut<ServerInputConfig<PlayerContext>>,
) {
    input_config.rebroadcast_inputs = **timeline == TimelinePolicy::AllPredicted;
}

fn action_started<A: InputAction>(actions: &Query<&ActionEvents, With<Action<A>>>) -> bool {
    // START is BEI's one-tick rising edge. FIRE remains set on every tick while
    // a bool action is held, which was why the old code needed a manual latch.
    actions
        .iter()
        .any(|events| events.contains(ActionEvents::START))
}

/// Apply requested axis selections, then reset the single arena.
///
/// Resetting is intentionally blunt and easy to reason about: all players,
/// projectile state, and player action entities are destroyed, then one fresh
/// player is spawned for every connected peer. There are no rooms and no epoch
/// identifiers.
#[allow(clippy::too_many_arguments)]
fn apply_global_actions_and_reset_arena(
    mut config: Single<
        (
            &mut TrajectoryKind,
            &mut RepresentationKind,
            &mut HitPolicy,
            &mut TimelinePolicy,
        ),
        With<ClientContext>,
    >,
    trajectory_actions: Query<&ActionEvents, With<Action<CycleTrajectory>>>,
    representation_actions: Query<&ActionEvents, With<Action<CycleRepresentation>>>,
    hit_actions: Query<&ActionEvents, With<Action<CycleHitPolicy>>>,
    timeline_actions: Query<&ActionEvents, With<Action<CycleTimeline>>>,
    mut input_config: ResMut<ServerInputConfig<PlayerContext>>,
    arena_entities: Query<
        Entity,
        Or<(
            With<PlayerMarker>,
            With<BulletMarker>,
            With<FireData>,
            With<HitImpact>,
            With<LagCompensatedSilhouette>,
        )>,
    >,
    clients: Query<(Entity, &RemoteId, Has<BotClient>), (With<ClientOf>, With<Connected>)>,
    mut commands: Commands,
) {
    let (trajectory, representation, hit_policy, timeline) = &mut *config;
    let mut changed = false;

    if action_started(&trajectory_actions) {
        **trajectory = trajectory.next();
        changed = true;
    }
    if action_started(&representation_actions) {
        **representation = representation.next();
        changed = true;
    }
    if action_started(&hit_actions) {
        **hit_policy = hit_policy.next();
        changed = true;
    }
    if action_started(&timeline_actions) {
        **timeline = timeline.next();
        changed = true;
    }
    if !changed {
        return;
    }

    input_config.rebroadcast_inputs = **timeline == TimelinePolicy::AllPredicted;
    let hit_policy = **hit_policy;
    let timeline = **timeline;

    info!(
        trajectory = trajectory.name(),
        representation = representation.name(),
        hit_policy = hit_policy.name(),
        timeline = timeline.name(),
        "Axis selection changed; resetting projectile arena"
    );

    for entity in &arena_entities {
        commands.entity(entity).try_despawn();
    }
    for (link, client_id, is_bot) in &clients {
        spawn_player_for_link(
            &mut commands,
            link,
            client_id.0,
            is_bot,
            timeline,
            hit_policy,
        );
    }
}

pub(crate) fn spawn_player(
    trigger: On<Add, Connected>,
    client: Query<(&RemoteId, Has<BotClient>), With<ClientOf>>,
    config: Single<(&TimelinePolicy, &HitPolicy), With<ClientContext>>,
    mut commands: Commands,
) {
    let Ok((client_id, is_bot)) = client.get(trigger.entity) else {
        return;
    };
    let (timeline, hit_policy) = *config;
    spawn_player_for_link(
        &mut commands,
        trigger.entity,
        client_id.0,
        is_bot,
        *timeline,
        *hit_policy,
    );
}

fn spawn_player_for_link(
    commands: &mut Commands,
    link: Entity,
    client_id: PeerId,
    is_bot: bool,
    timeline: TimelinePolicy,
    hit_policy: HitPolicy,
) {
    let mut player = commands.spawn((
        shared::player_bundle(client_id, is_bot),
        Replicate::to_clients(NetworkTarget::All),
        ControlledBy {
            owner: link,
            lifetime: Default::default(),
        },
    ));
    timeline.configure_player(&mut player, client_id);
    if hit_policy == HitPolicy::ServerRewound {
        player.insert(LagCompensationHistory::default());
    }
    if is_bot {
        player.insert(Bot);
    }
    let player = player.id();
    spawn_player_actions(commands, player);
    info!(?player, ?client_id, "Spawned one player for connected peer");
}

/// Accept the deliberately insecure client-reported result only while that
/// policy is selected. Geometry is intentionally not rechecked here.
fn handle_client_reported_hit(
    trigger: On<RemoteEvent<HitDetected>>,
    players: Query<(), With<PlayerMarker>>,
    bots: Query<(), With<Bot>>,
    mut scores: Query<&mut Score, With<PlayerMarker>>,
    mut commands: Commands,
) {
    if !players.contains(trigger.trigger.target) {
        return;
    }
    accept_hit(
        &mut commands,
        trigger.trigger.shooter,
        trigger.trigger.target,
        trigger.trigger.impact,
        &bots,
        &mut scores,
    );
    debug!(
        shooter = ?trigger.trigger.shooter,
        target = ?trigger.trigger.target,
        "Accepted client-reported hit"
    );
}
