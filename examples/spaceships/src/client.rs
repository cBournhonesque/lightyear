use bevy::app::PluginGroupBuilder;
use bevy::prelude::*;
use bevy::utils::Duration;
use bevy_xpbd_2d::prelude::*;
use leafwing_input_manager::prelude::*;
use lightyear::prelude::client::*;
use lightyear::prelude::*;
use lightyear::shared::replication::components::Controlled;
use lightyear::shared::tick_manager;
use lightyear_examples_common::shared::FIXED_TIMESTEP_HZ;

use crate::protocol::*;
use crate::shared;
use crate::shared::ApplyInputsQuery;
use crate::shared::{color_from_id, shared_movement_behaviour, FixedSet};

pub struct ExampleClientPlugin;

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, init);
        app.add_systems(
            PreUpdate,
            handle_connection
                .after(MainSet::Receive)
                .before(PredictionSet::SpawnPrediction),
        );
        // all actions related-system that can be rolled back should be in FixedUpdate schedule
        app.add_systems(
            FixedUpdate,
            (
                player_movement,
                // we don't spawn bullets during rollback.
                // if we have the inputs early (so not in rb) then we spawn,
                // otherwise we rely on normal server replication to spawn them
                shared::shared_player_firing.run_if(not(is_in_rollback)),
            )
                .chain()
                .in_set(FixedSet::Main),
        );
        app.add_systems(
            Update,
            (
                add_ball_physics,
                add_bullet_physics, // TODO better to scheduled right after replicated entities get spawned?
                handle_new_player,
            ),
        );
        #[cfg(target_family = "wasm")]
        app.add_systems(
            Startup,
            |mut settings: ResMut<lightyear::client::web::KeepaliveSettings>| {
                // the show must go on, even in the background.
                let keepalive = 1000. / FIXED_TIMESTEP_HZ;
                info!("Setting webworker keepalive to {keepalive}");
                settings.wake_delay = keepalive;
            },
        );
    }
}

// Startup system for the client
pub(crate) fn init(mut commands: Commands) {
    commands.connect_client();
}

/// Listen for events to know when the client is connected, and spawn a text entity
/// to display the client id
pub(crate) fn handle_connection(
    mut commands: Commands,
    mut connection_event: EventReader<ConnectEvent>,
) {
    for event in connection_event.read() {
        let client_id = event.client_id();
        commands.spawn(TextBundle::from_section(
            format!("Client {}", client_id),
            TextStyle {
                font_size: 30.0,
                color: Color::WHITE,
                ..default()
            },
        ));
    }
}

/// Blueprint pattern: when the ball gets replicated from the server, add all the components
/// that we need that are not replicated.
/// (for example physical properties that are constant, so they don't need to be networked)
///
/// We only add the physical properties on the ball that is displayed on screen (i.e the Interpolated ball)
/// We want the ball to be rigid so that when players collide with it, they bounce off.
///
/// However we remove the Position because we want the balls position to be interpolated, without being computed/updated
/// by the physics engine? Actually this shouldn't matter because we run interpolation in PostUpdate...
fn add_ball_physics(
    mut commands: Commands,
    mut ball_query: Query<Entity, (With<BallMarker>, Added<Predicted>)>,
) {
    for entity in ball_query.iter_mut() {
        info!("Adding physics to a replicated ball {entity:?}");
        commands.entity(entity).insert(PhysicsBundle::ball());
    }
}

/// Simliar blueprint scenario as balls, except sometimes clients prespawn bullets ahead of server
/// replication, which means they will already have the physics components.
/// So, we filter the query using `Without<Collider>`.
fn add_bullet_physics(
    mut commands: Commands,
    mut bullet_query: Query<Entity, (With<BulletMarker>, Added<Predicted>, Without<Collider>)>,
) {
    for entity in bullet_query.iter_mut() {
        info!("Adding physics to a replicated bullet:  {entity:?}");
        commands.entity(entity).insert(PhysicsBundle::bullet());
    }
}

/// Decorate newly connecting players with physics components
/// ..and if it's our own player, set up input stuff
#[allow(clippy::type_complexity)]
fn handle_new_player(
    connection: Res<ClientConnection>,
    mut commands: Commands,
    mut player_query: Query<(Entity, Has<Controlled>), (Added<Predicted>, With<Player>)>,
) {
    for (entity, is_controlled) in player_query.iter_mut() {
        // is this our own entity?
        if is_controlled {
            info!("Own player replicated to us, adding inputmap {entity:?}");
            commands
                .entity(entity)
                .insert(InputMap::new([
                    (PlayerActions::Up, KeyCode::ArrowUp),
                    (PlayerActions::Down, KeyCode::ArrowDown),
                    (PlayerActions::Left, KeyCode::ArrowLeft),
                    (PlayerActions::Right, KeyCode::ArrowRight),
                    (PlayerActions::Up, KeyCode::KeyW),
                    (PlayerActions::Down, KeyCode::KeyS),
                    (PlayerActions::Left, KeyCode::KeyA),
                    (PlayerActions::Right, KeyCode::KeyD),
                    (PlayerActions::Fire, KeyCode::Space),
                ]))
                .insert(ActionState::<PlayerActions>::default());
        } else {
            info!("Remote player replicated to us: {entity:?}");
        }
        let client_id = connection.id();
        info!(?entity, ?client_id, "adding physics to predicted player");
        commands.entity(entity).insert(PhysicsBundle::player_ship());
    }
}

// only apply movements to predicted entities
fn player_movement(
    mut q: Query<ApplyInputsQuery, (With<Player>, With<Predicted>)>,
    tick_manager: Res<TickManager>,
    rollback: Res<Rollback>,
) {
    for aiq in q.iter_mut() {
        shared_movement_behaviour(aiq);
    }
}
