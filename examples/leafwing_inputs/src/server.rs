use crate::protocol::*;
use crate::shared::{color_from_id, shared_config, shared_movement_behaviour};
use crate::{shared, Transports, KEY, PROTOCOL_ID};
use bevy::prelude::*;
use bevy_xpbd_2d::prelude::*;
use leafwing_input_manager::prelude::*;
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

#[derive(Resource, Clone, Copy)]
pub struct MyServerPlugin {
    pub(crate) port: u16,
    pub(crate) transport: Transports,
    /// If this is true, we will predict the client's entities, but also the ball and other clients' entities!
    /// This is what is done by RocketLeague (see [video](https://www.youtube.com/watch?v=ueEmiDM94IE))
    ///
    /// If this is false, we will predict the client's entites but simple interpolate everything else.
    pub(crate) predict_all: bool,
}

impl Plugin for MyServerPlugin {
    fn build(&self, app: &mut App) {
        let server_addr = SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), self.port);
        let netcode_config = NetcodeConfig::default()
            .with_protocol_id(PROTOCOL_ID)
            .with_key(KEY);
        let link_conditioner = LinkConditionerConfig {
            incoming_latency: Duration::from_millis(75),
            incoming_jitter: Duration::from_millis(0),
            incoming_loss: 0.0,
        };
        let transport = match self.transport {
            Transports::Udp => TransportConfig::UdpSocket(server_addr),
            Transports::Webtransport => TransportConfig::WebTransportServer {
                server_addr,
                certificate: Certificate::self_signed(&["localhost"]),
            },
        };
        let io =
            Io::from_config(IoConfig::from_transport(transport).with_conditioner(link_conditioner));
        let config = ServerConfig {
            shared: shared_config().clone(),
            netcode: netcode_config,
            ping: PingConfig::default(),
        };
        let plugin_config = PluginConfig::new(config, io, protocol());
        app.add_plugins(server::ServerPlugin::new(plugin_config));
        app.add_plugins(shared::SharedPlugin);
        // add leafwing plugins to handle inputs
        app.add_plugins(LeafwingInputPlugin::<MyProtocol, PlayerActions>::default());
        app.add_plugins(LeafwingInputPlugin::<MyProtocol, AdminActions>::default());
        app.insert_resource(self.clone());
        app.add_systems(Startup, init);
        // Re-adding Replicate components to client-replicated entities must be done in this set for proper handling.
        app.add_systems(
            PreUpdate,
            (replicate_players).in_set(MainSet::ClientReplication),
        );
        // the physics/FixedUpdates systems that consume inputs should be run in this set
        app.add_systems(
            FixedUpdate,
            (movement)
                .in_set(FixedUpdateSet::Main)
                .before(PhysicsSet::Prepare),
        );
        app.add_systems(Update, handle_disconnections);
    }
}

pub(crate) fn init(mut commands: Commands, plugin: Res<MyServerPlugin>) {
    commands.spawn(Camera2dBundle::default());
    commands.spawn(
        TextBundle::from_section(
            "Server",
            TextStyle {
                font_size: 30.0,
                color: Color::WHITE,
                ..default()
            },
        )
        .with_style(Style {
            align_self: AlignSelf::End,
            ..default()
        }),
    );

    // the ball is server-authoritative
    commands.spawn(BallBundle::new(
        Vec2::new(0.0, 0.0),
        Color::AZURE,
        // if true, we predict the ball on clients
        plugin.predict_all,
    ));
}

/// Server disconnection system, delete all player entities upon disconnection
pub(crate) fn handle_disconnections(
    mut disconnections: EventReader<DisconnectEvent>,
    mut commands: Commands,
    player_entities: Query<(Entity, &PlayerId)>,
) {
    for disconnection in disconnections.read() {
        let client_id = disconnection.context();
        for (entity, player_id) in player_entities.iter() {
            if player_id.0 == *client_id {
                commands.entity(entity).despawn();
            }
        }
    }
}

/// Read client inputs and move players
/// NOTE: this system can now be run in both client/server!
pub(crate) fn movement(
    server: Res<Server>,
    mut action_query: Query<(
        Entity,
        &Position,
        &mut LinearVelocity,
        &ActionState<PlayerActions>,
    )>,
) {
    for (entity, position, velocity, action) in action_query.iter_mut() {
        // NOTE: be careful to directly pass Mut<PlayerPosition>
        // getting a mutable reference triggers change detection, unless you use `as_deref_mut()`
        shared_movement_behaviour(velocity, action);
        info!(?entity, tick = ?server.tick(), ?position, actions = ?action.get_pressed(), "applying movement to player");
    }
}

// Replicate the pre-spawned entities back to the client

pub(crate) fn replicate_players(
    plugin: Res<MyServerPlugin>,
    mut commands: Commands,
    mut player_spawn_reader: EventReader<ComponentInsertEvent<PlayerId>>,
) {
    for event in player_spawn_reader.read() {
        debug!("received player spawn event: {:?}", event);
        let client_id = event.context();
        let entity = event.entity();

        // for all cursors we have received, add a Replicate component so that we can start replicating it
        // to other clients

        if let Some(mut e) = commands.get_entity(entity) {
            let mut replicate = Replicate {
                // we want to replicate back to the original client, since they are using a pre-spawned entity
                replication_target: NetworkTarget::All,
                // make sure that all entities that are predicted are part of the same replication group
                replication_group: REPLICATION_GROUP,
                ..default()
            };
            // We don't want to replicate the ActionState to the original client, since they are updating it with
            // their own inputs (if you replicate it to the original client, it will be added on the Confirmed entity,
            // which will keep syncing it to the Predicted entity because the ActionState gets updated every tick)!
            replicate.add_target::<ActionState<PlayerActions>>(NetworkTarget::AllExcept(vec![
                *client_id,
            ]));
            if plugin.predict_all {
                replicate.prediction_target = NetworkTarget::All;
                // if we predict other players, we need to replicate their actions to all clients other than the original one
                // (the original client will apply the actions locally)
                replicate.disable_replicate_once::<ActionState<PlayerActions>>();
            } else {
                // NOTE: even with a pre-spawned Predicted entity, we need to specify who will run prediction
                replicate.prediction_target = NetworkTarget::Only(vec![*client_id]);
                // we want the other clients to apply interpolation for the player
                replicate.interpolation_target = NetworkTarget::AllExcept(vec![*client_id]);
            }
            e.insert((
                replicate,
                // not all physics components are replicated over the network, so add them on the server as well
                PhysicsBundle::player(),
            ));
        }
    }
}
