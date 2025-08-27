use crate::protocol::*;
use crate::shared;
use crate::shared::{color_from_id, Rooms, SharedPlugin, BOT_RADIUS};
use avian2d::prelude::*;
use bevy::prelude::*;
use bevy::time::Stopwatch;
use core::ops::DerefMut;
use core::time::Duration;
use std::net::SocketAddr;
use bevy::input::InputPlugin;
use bevy_enhanced_input::EnhancedInputSet;
use bevy_enhanced_input::prelude::{Completed, Started};
use leafwing_input_manager::prelude::*;
use lightyear::core::tick::TickDuration;
use lightyear::crossbeam::CrossbeamIo;
use lightyear::interpolation::plugin::InterpolationDelay;
use lightyear::netcode::NetcodeClient;
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use lightyear_avian2d::prelude::{
    LagCompensationHistory, LagCompensationPlugin, LagCompensationSet, LagCompensationSpatialQuery,
};
use lightyear_examples_common::shared::{SEND_INTERVAL, SERVER_ADDR, SHARED_SETTINGS};
use rand::random;
use lightyear_examples_common::cli::new_headless_app;
use crate::client::ExampleClientPlugin;

pub struct ExampleServerPlugin;

const BULLET_COLLISION_DISTANCE_CHECK: f32 = 4.0;

impl Plugin for ExampleServerPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(RoomPlugin);
        app.init_resource::<Rooms>();

        app.add_plugins(LagCompensationPlugin);
        app.add_observer(handle_new_client);
        app.add_observer(spawn_player);
        app.add_observer(cycle_replication_mode);
        app.add_observer(cycle_projectile_mode);

        app.add_systems(Startup, spawn_global_control);

        // we don't want to panic when trying to read the InputReader if gui is not enabled
        app.configure_sets(PreUpdate, EnhancedInputSet::Prepare.run_if(|| false));

        // the lag compensation systems need to run after LagCompensationSet::UpdateHistory
        app.add_systems(
            FixedUpdate,
            (
                interpolated_bot_movement,
                // room_cycling)
            ),
        );
        app.add_systems(
            PhysicsSchedule,
            // lag compensation collisions must run after the SpatialQuery has been updated
            compute_hit_lag_compensation.in_set(LagCompensationSet::Collisions),
        );
        app.add_systems(
            FixedPostUpdate,
            // check collisions after physics have run
            compute_hit_prediction.after(PhysicsSet::Sync),
        );

        app.add_plugins(BotPlugin);
    }
}

/// Spawn bots when the server starts
/// NOTE: this has to be done after `Plugin::finish()` so that BEI has finished building.
pub(crate) fn spawn_bots(mut commands: Commands) {
    commands.trigger(SpawnBot);
}


pub(crate) fn handle_new_client(trigger: Trigger<OnAdd, LinkOf>, mut commands: Commands) {
    commands.entity(trigger.target()).insert((
        ReplicationSender::new(SEND_INTERVAL, SendUpdatesMode::SinceLastAck, false),
        // We need a ReplicationReceiver on the server side because the Action entities are spawned
        // on the client and replicated to the server.
        ReplicationReceiver::default(),
        Name::from("ClientOf"),
    ));
}

pub(crate) fn spawn_global_control(
    mut commands: Commands,
) {
    commands.spawn((
        ClientContext,
        Replicate::to_clients(NetworkTarget::All),
        GameReplicationMode::default(),
        ProjectileReplicationMode::default(),
        Name::new("ClientContext"),
    ));
}

// Replicate the pre-spawned entities back to the client
// We have to use `InitialReplicated` instead of `Replicated`, because
// the server has already assumed authority over the entity so the `Replicated` component
// has been removed
pub(crate) fn spawn_player(
    trigger: Trigger<OnAdd, Connected>,
    query: Query<(&RemoteId, Has<BotClient>), With<ClientOf>>,
    mut commands: Commands,
    mut rooms: ResMut<Rooms>,
    replicated_players: Query<
        (Entity, &InitialReplicated),
        (Added<InitialReplicated>, With<PlayerId>),
    >,
) {
    let sender = trigger.target();
    let Ok((client_id, is_bot)) = query.get(sender) else {
        return;
    };
    let client_id = client_id.0;
    info!("Spawning player with id: {}", client_id);

    for i in 0..6 {
        let replication_mode = GameReplicationMode::from_room_id(i);
        let room = *rooms.rooms.entry(replication_mode)
            .or_insert_with(|| {
                 commands
                    .spawn(
                    (
                            Room::default(),
                            Name::new(format!("Room{}", replication_mode.name())),
                        )
                ).id()
            });

        // start by adding the player to the first room
        if i == 0 {
            commands
                .entity(room)
                .trigger(RoomEvent::AddSender(trigger.target()));
        }
        let player = server_player_bundle(room, client_id, sender, replication_mode);
        let player_entity = match replication_mode {
            GameReplicationMode::AllPredicted => {
                commands.spawn((
                    player,
                    PredictionTarget::to_clients(NetworkTarget::All),
                ))
            }
            GameReplicationMode::ClientPredictedNoComp => commands.spawn((
                player,
                PredictionTarget::to_clients(NetworkTarget::Single(client_id)),
                InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_id)),
            )),
            GameReplicationMode::ClientPredictedLagComp => commands.spawn((
                player,
                PredictionTarget::to_clients(NetworkTarget::Single(client_id)),
                InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_id)),
            )),
            GameReplicationMode::ClientSideHitDetection => commands.spawn((
                player,
                PredictionTarget::to_clients(NetworkTarget::Single(client_id)),
                InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_id)),
            )),
            GameReplicationMode::AllInterpolated => {
                commands.spawn((
                    player,
                    InterpolationTarget::to_clients(NetworkTarget::All),
                ))
            }
            GameReplicationMode::OnlyInputsReplicated => commands.spawn((
                PlayerContext,
                Replicate::to_clients(NetworkTarget::All),
                NetworkVisibility::default(),
                ControlledBy {
                    owner: sender,
                    lifetime: Default::default(),
                },
                PlayerMarker,
                Name::new("Player"),
            )),
        }
        .id();
        if is_bot {
            commands.entity(player_entity).insert(Bot);
        }
        info!("Spawning player {player_entity:?} for room: {room:?}");
        commands
            .entity(room)
            .trigger(RoomEvent::AddEntity(player_entity));
    }
}

fn server_player_bundle(room: Entity, client_id: PeerId, owner: Entity, replication_mode: GameReplicationMode) -> impl Bundle {
    let bundle = shared::player_bundle(client_id);
    (
        Replicate::to_clients(NetworkTarget::All),
        NetworkVisibility::default(),
        ControlledBy {
            owner,
            lifetime: Default::default(),
        },
        replication_mode,
        bundle
    )
}


/// Compute hits if the bullet hits the bot, and increment the score on the player
pub(crate) fn compute_hit_lag_compensation(
    // instead of directly using avian's SpatialQuery, we want to use the LagCompensationSpatialQuery
    // to apply lag-compensation (i.e. compute the collision between the bullet and the collider as it
    // was seen by the client when they fired the shot)
    mut commands: Commands,
    timeline: Single<&LocalTimeline, With<Server>>,
    query: LagCompensationSpatialQuery,
    bullets: Query<
        (Entity, &PlayerId, &Position, &LinearVelocity, &ControlledBy),
        With<BulletMarker>,
    >,
    // the InterpolationDelay component is stored directly on the client entity
    // (the server creates one entity for each client to store client-specific
    // metadata)
    client_query: Query<&InterpolationDelay, With<ClientOf>>,
    mut player_query: Query<(&mut Score, &PlayerId), With<PlayerMarker>>,
) {
    let tick = timeline.tick();
    bullets
        .iter()
        .for_each(|(entity, id, position, velocity, controlled_by)| {
            let Ok(delay) = client_query.get(controlled_by.owner) else {
                error!("Could not retrieve InterpolationDelay for client {id:?}");
                return;
            };
            if let Some(hit_data) = query.cast_ray(
                // the delay is sent in every input message; the latest InterpolationDelay received
                // is stored on the client entity
                *delay,
                position.0,
                Dir2::new_unchecked(velocity.0.normalize()),
                // TODO: shouldn't this be based on velocity length?
                BULLET_COLLISION_DISTANCE_CHECK,
                false,
                &mut SpatialQueryFilter::default(),
            ) {
                info!(
                    ?tick,
                    ?hit_data,
                    ?entity,
                    "Collision with interpolated bot! Despawning bullet"
                );
                // if there is a hit, increment the score
                player_query
                    .iter_mut()
                    .find(|(_, player_id)| player_id.0 == id.0)
                    .map(|(mut score, _)| {
                        score.0 += 1;
                    });
                commands.entity(entity).despawn();
            }
        })
}

pub(crate) fn compute_hit_prediction(
    mut commands: Commands,
    timeline: Single<&LocalTimeline, With<Server>>,
    query: SpatialQuery,
    bullets: Query<(Entity, &PlayerId, &Position, &LinearVelocity), With<BulletMarker>>,
    bot_query: Query<(), (With<PredictedBot>, Without<Confirmed>)>,
    // the InterpolationDelay component is stored directly on the client entity
    // (the server creates one entity for each client to store client-specific
    // metadata)
    mut player_query: Query<(&mut Score, &PlayerId), With<PlayerMarker>>,
) {
    let tick = timeline.tick();
    bullets.iter().for_each(|(entity, id, position, velocity)| {
        if let Some(hit_data) = query.cast_ray_predicate(
            position.0,
            Dir2::new_unchecked(velocity.0.normalize()),
            // TODO: shouldn't this be based on velocity length?
            BULLET_COLLISION_DISTANCE_CHECK,
            false,
            &SpatialQueryFilter::default(),
            &|entity| {
                // only confirm the hit on predicted bots
                bot_query.get(entity).is_ok()
            },
        ) {
            info!(
                ?tick,
                ?hit_data,
                ?entity,
                "Collision with predicted bot! Despawn bullet"
            );
            // if there is a hit, increment the score
            player_query
                .iter_mut()
                .find(|(_, player_id)| player_id.0 == id.0)
                .map(|(mut score, _)| {
                    score.0 += 1;
                });
            commands.entity(entity).despawn();
        }
    })
}

fn interpolated_bot_movement(
    timeline: Single<&LocalTimeline, With<Server>>,
    mut query: Query<&mut Position, With<InterpolatedBot>>,
) {
    let tick = timeline.tick();
    query.iter_mut().for_each(|mut position| {
        // change direction every 200ticks
        let direction = if (tick.0 / 200) % 2 == 0 { 1.0 } else { -1.0 };
        position.x += shared::BOT_MOVE_SPEED * direction;
    });
}

pub struct BotPlugin;

impl Plugin for BotPlugin {
    fn build(&self, app: &mut App) {
    }

    // run in `cleanup` because BEI finishes building in `finish`
    fn cleanup(&self, app: &mut App) {
        app.add_observer(spawn_bot_app);
        app.add_systems(Startup, spawn_bots);
    }
}

#[derive(Component)]
pub struct BotClient;

#[derive(Event)]
pub struct SpawnBot;

pub struct BotApp(App);

unsafe impl Send for BotApp {}
unsafe impl Sync for BotApp {}

impl BotApp {
    fn run(&mut self) {
        self.0.run();
    }
}


/// On the server, we will create a second app to host a bot that is similar to a real client,
/// but their inputs are mocked
fn spawn_bot_app(
    trigger: Trigger<SpawnBot>,
    tick_duration: Res<TickDuration>,
    server: Single<Entity, With<Server>>,
    mut commands: Commands,
) {
    info!("Spawning bot app");
    let (crossbeam_client, crossbeam_server) = CrossbeamIo::new_pair();

    let mut app = new_headless_app();
    // TODO: just spawn a bot player entity without creating a new client
    // cannot use headless app because the frame rate is too fast so
    // the bot sends too many packets
    // let mut app = new_gui_app(false);
    app.add_plugins(InputPlugin);
    app.add_plugins(lightyear::prelude::client::ClientPlugins {
        tick_duration: tick_duration.0,
    });
    app.add_plugins(SharedPlugin);
    app.add_plugins(ExampleClientPlugin);

    let client_id = rand::random::<u64>();
    let auth = Authentication::Manual {
        server_addr: SERVER_ADDR,
        client_id,
        private_key: SHARED_SETTINGS.private_key,
        protocol_id: SHARED_SETTINGS.protocol_id,
    };

    app.world_mut().spawn((
        Client::default(),
        BotClient,
        ReplicationSender::new(
            SEND_INTERVAL,
            SendUpdatesMode::SinceLastAck,
            false,
        ),
        ReplicationReceiver::default(),
        NetcodeClient::new(
            auth,
            lightyear::netcode::client_plugin::NetcodeConfig::default(),
        )
        .unwrap(),
        crossbeam_client,
        PredictionManager::default(),
        InterpolationManager::default(),
        Name::from("BotClient"),
    ));
    let server = server.into_inner();
    let conditioner = RecvLinkConditioner::new(LinkConditionerConfig::average_condition());
    commands.spawn((
        LinkOf { server },
        Link::new(Some(conditioner)),
        Linked,
        ClientOf,
        BotClient,
        crossbeam_server,
        ReplicationSender::default(),
    ));

    app.add_systems(Startup, bot_connect);
    app.add_systems(First, bot_inputs);
    #[cfg(not(feature = "gui"))]
    app.add_systems(Update, bot_wait);
    let mut bot_app = BotApp(app);
    std::thread::spawn(move || {
        bot_app.run();
    });
}

fn bot_connect(
    bot: Single<Entity, (With<BotClient>, With<Client>)>,
    mut commands: Commands
) {
    let entity = bot.into_inner();
    info!("Bot entity {entity:?} connecting to server");
    commands.entity(entity).trigger(Connect);
}

#[derive(Debug, Clone, Copy, Default)]
enum BotMovementMode {
    #[default]
    Strafing,    // 200ms intervals
    StraightLine, // 1s intervals
}

impl BotMovementMode {
    fn interval(&self) -> f32 {
        match self {
            BotMovementMode::Strafing => 0.2,     // 200ms for strafing
            BotMovementMode::StraightLine => 1.0, // 1s for straight line
        }
    }

    fn name(&self) -> &'static str {
        match self {
            BotMovementMode::Strafing => "strafing",
            BotMovementMode::StraightLine => "straight-line",
        }
    }
}

fn bot_inputs(
    time: Res<Time>,
    mut input: ResMut<ButtonInput<KeyCode>>,
    mut local: Local<(Stopwatch, Stopwatch, BotMovementMode, bool)>,
) {
    let (mode_timer, key_timer, current_mode, press_a) = local.deref_mut();

    mode_timer.tick(time.delta());
    key_timer.tick(time.delta());

    // Switch modes every 4 seconds
    if mode_timer.elapsed_secs() >= 4.0 {
        mode_timer.reset();
        *current_mode = match *current_mode {
            BotMovementMode::Strafing => BotMovementMode::StraightLine,
            BotMovementMode::StraightLine => BotMovementMode::Strafing,
        };
        trace!("Bot switching to {} mode", current_mode.name());
    }

    // Switch keys based on the current mode's interval
    if key_timer.elapsed_secs() >= current_mode.interval() {
        key_timer.reset();
        if *press_a {
            input.release(KeyCode::KeyA);
        } else {
            input.release(KeyCode::KeyD);
        }
        *press_a = !*press_a;
    }

    // Press the current key
    if *press_a {
        input.press(KeyCode::KeyA);
    } else {
        input.press(KeyCode::KeyD);
    }

    trace!("Bot in {} mode, pressing {:?}",
           current_mode.name(),
           input.get_pressed().collect::<Vec<_>>());
}

#[cfg(not(feature = "gui"))]
// prevent the bot from running too fast
fn bot_wait(
    timeline: Single<&LocalTimeline>)
{
    std::thread::sleep(Duration::from_millis(15));
}

/// Handle room switching when replication mode changes
pub fn cycle_replication_mode(
    trigger: Trigger<Completed<CycleReplicationMode>>,
    mut global: Query<&mut GameReplicationMode, With<ClientContext>>,
    rooms: Res<Rooms>,
    clients: Query<Entity, With<ClientOf>>,
    mut commands: Commands,
) {
        if let Ok(mut replication_mode) = global.single_mut() {
        let current_mode = *replication_mode;
        *replication_mode = replication_mode.next();

        // Move all clients from current room to next room
        if let (Some(current_room), Some(next_room)) = (
            rooms.rooms.get(&current_mode),
            rooms.rooms.get(&*replication_mode)
        ) {
            for client_entity in clients.iter() {
                commands.trigger_targets(RoomEvent::RemoveSender(client_entity), *current_room);
                commands.trigger_targets(RoomEvent::AddSender(client_entity), *next_room);
                info!("Switching client {client_entity:?} from room {current_room:?} to room {next_room:?}");
            }
        }

        info!("Cycled to replication mode: {}", replication_mode.name());
    }
}

/// Handle cycling through projectile replication modes
pub fn cycle_projectile_mode(
    trigger: Trigger<Completed<CycleProjectileMode>>,
    mut global: Query<&mut ProjectileReplicationMode, With<ClientContext>>,
) {
    if let Ok(mut projectile_mode) = global.single_mut() {
        *projectile_mode = projectile_mode.next();
        info!("Cycled to projectile mode: {}", projectile_mode.name());
    }
}


