use crate::automation::AutomationServerPlugin;
use crate::bot::BotClient;
#[cfg(feature = "client")]
use crate::bot::BotPlugin;
use crate::hit_detection::{
    HitImpact, HitPolicy, hit_policy_is, server_current,
    server_rewound::{self, LagCompensatedSilhouette},
};
use crate::protocol::*;
use crate::representation::{RepresentationKind, fire_data_entity::FireData};
use crate::shared;
use crate::timeline::TimelinePolicy;
use crate::trajectory::TrajectoryKind;
use avian2d::prelude::*;
use bevy::platform::collections::HashSet;
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
        app.init_resource::<GlobalActionLatch>();
        app.insert_resource(ReplicationMetadata::new(SEND_INTERVAL));
        app.add_plugins(LagCompensationPlugin);

        app.add_observer(handle_new_client);
        app.add_observer(spawn_player);
        app.add_observer(release_global_action::<CycleTrajectory>);
        app.add_observer(release_global_action::<CycleRepresentation>);
        app.add_observer(release_global_action::<CycleHitPolicy>);
        app.add_observer(release_global_action::<CycleTimeline>);
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
fn spawn_player_actions(commands: &mut Commands, player: Entity, is_bot: bool) {
    commands.spawn((
        ActionOf::<PlayerContext>::new(player),
        Action::<MovePlayer>::new(),
        ReplicateLike { root: player },
    ));
    // The bot deliberately has no aim action: its initial downward rotation is
    // fixed, so all peers simulate the same simple strafing firing target.
    if !is_bot {
        commands.spawn((
            ActionOf::<PlayerContext>::new(player),
            Action::<MoveCursor>::new(),
            ReplicateLike { root: player },
        ));
    }
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

fn env_value(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_ascii_lowercase().replace(['-', ' '], "_"))
}

fn initial_trajectory() -> TrajectoryKind {
    match env_value("LIGHTYEAR_INITIAL_TRAJECTORY").as_deref() {
        None | Some("hitscan" | "hit_scan" | "0") => TrajectoryKind::Hitscan,
        Some("linear" | "bullet" | "linear_projectile" | "1") => TrajectoryKind::Linear,
        Some(value) => {
            warn!(value, "Ignoring unknown LIGHTYEAR_INITIAL_TRAJECTORY");
            TrajectoryKind::default()
        }
    }
}

fn initial_representation() -> RepresentationKind {
    match env_value("LIGHTYEAR_INITIAL_REPRESENTATION").as_deref() {
        None | Some("state" | "state_entity" | "full" | "0") => RepresentationKind::StateEntity,
        Some("fire_data" | "fire_data_entity" | "direction" | "1") => {
            RepresentationKind::FireDataEntity
        }
        Some("shot_buffer" | "buffer" | "2") => RepresentationKind::ShotBuffer,
        Some(value) => {
            warn!(value, "Ignoring unknown LIGHTYEAR_INITIAL_REPRESENTATION");
            RepresentationKind::default()
        }
    }
}

fn initial_hit_policy() -> HitPolicy {
    match env_value("LIGHTYEAR_INITIAL_HIT_POLICY").as_deref() {
        None | Some("server_current" | "current" | "0") => HitPolicy::ServerCurrent,
        Some("server_rewound" | "rewound" | "lag_comp" | "1") => HitPolicy::ServerRewound,
        Some("client_reported" | "client" | "2") => HitPolicy::ClientReported,
        Some(value) => {
            warn!(value, "Ignoring unknown LIGHTYEAR_INITIAL_HIT_POLICY");
            HitPolicy::default()
        }
    }
}

fn initial_timeline() -> TimelinePolicy {
    match env_value("LIGHTYEAR_INITIAL_TIMELINE").as_deref() {
        None | Some("owner_predicted" | "default" | "0") => {
            TimelinePolicy::OwnerPredictedRemotesInterpolated
        }
        Some("all_predicted" | "predicted" | "1") => TimelinePolicy::AllPredicted,
        Some("all_interpolated" | "interpolated" | "2") => TimelinePolicy::AllInterpolated,
        Some(value) => {
            warn!(value, "Ignoring unknown LIGHTYEAR_INITIAL_TIMELINE");
            TimelinePolicy::default()
        }
    }
}

#[derive(Resource, Default)]
struct GlobalActionLatch {
    active: HashSet<Entity>,
}

impl GlobalActionLatch {
    fn start(&mut self, action: Entity) -> bool {
        self.active.insert(action)
    }

    fn complete(&mut self, action: Entity) {
        self.active.remove(&action);
    }
}

fn release_global_action<A: InputAction>(
    trigger: On<Complete<A>>,
    mut latch: ResMut<GlobalActionLatch>,
) {
    latch.complete(trigger.action);
}

fn take_fired_once<A: InputAction>(
    actions: &Query<(Entity, &ActionEvents), With<Action<A>>>,
    latch: &mut GlobalActionLatch,
) -> bool {
    let mut fired = false;
    for (entity, events) in actions {
        if events.contains(ActionEvents::COMPLETE) || events.contains(ActionEvents::CANCEL) {
            latch.complete(entity);
        }
        if events.contains(ActionEvents::FIRE) && latch.start(entity) {
            fired = true;
        }
    }
    fired
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
    trajectory_actions: Query<(Entity, &ActionEvents), With<Action<CycleTrajectory>>>,
    representation_actions: Query<(Entity, &ActionEvents), With<Action<CycleRepresentation>>>,
    hit_actions: Query<(Entity, &ActionEvents), With<Action<CycleHitPolicy>>>,
    timeline_actions: Query<(Entity, &ActionEvents), With<Action<CycleTimeline>>>,
    mut latch: ResMut<GlobalActionLatch>,
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

    if take_fired_once(&trajectory_actions, &mut latch) {
        **trajectory = trajectory.next();
        changed = true;
    }
    if take_fired_once(&representation_actions, &mut latch) {
        **representation = representation.next();
        changed = true;
    }
    if take_fired_once(&hit_actions, &mut latch) {
        **hit_policy = hit_policy.next();
        changed = true;
    }
    if take_fired_once(&timeline_actions, &mut latch) {
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
    spawn_player_actions(commands, player, is_bot);
    info!(?player, ?client_id, "Spawned one player for connected peer");
}

/// Accept the deliberately insecure client-reported result only while that
/// policy is selected. Geometry is intentionally not rechecked here.
fn handle_client_reported_hit(
    trigger: On<RemoteEvent<HitDetected>>,
    players: Query<(), With<PlayerMarker>>,
    mut scores: Query<&mut Score, With<PlayerMarker>>,
) {
    if !players.contains(trigger.trigger.target) {
        return;
    }
    if let Ok(mut score) = scores.get_mut(trigger.trigger.shooter) {
        score.0 += 1;
        debug!(
            shooter = ?trigger.trigger.shooter,
            target = ?trigger.trigger.target,
            score = score.0,
            "Accepted client-reported hit"
        );
    }
}
