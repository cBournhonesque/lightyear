use avian3d::prelude::*;
use bevy::prelude::*;
use bevy_ahoy::CharacterControllerState;
use lightyear::{
    connection::{client::Connected, client_of::ClientOf},
    prelude::input::native::ActionState,
    prelude::server::*,
    prelude::*,
};
use lightyear_ahoy::prelude::AhoyUserCommand;

use crate::{protocol::*, shared::*};

#[derive(Clone)]
pub struct ExampleServerPlugin;

impl Plugin for ExampleServerPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(ReplicationMetadata::default());
        app.add_observer(handle_new_client);
        app.add_observer(handle_connected);
        app.add_systems(FixedPostUpdate, reset_fallen_players);
    }
}

fn handle_new_client(trigger: On<Add, LinkOf>, mut commands: Commands) {
    commands.entity(trigger.entity).insert(ReplicationSender);
}

fn handle_connected(
    trigger: On<Add, Connected>,
    clients: Query<&RemoteId, With<ClientOf>>,
    players: Query<&PlayerMarker>,
    mut commands: Commands,
) {
    let Ok(client_id) = clients.get(trigger.entity) else {
        return;
    };
    let client_id = client_id.0;
    let slot = players.iter().count() as u64 + 1;

    let player = commands
        .spawn((
            player_bundle(client_id, slot),
            ahoy_player_bundle(),
            ActionState::<AhoyUserCommand>::default(),
            Replicate::to_clients(NetworkTarget::All),
            PredictionTarget::to_clients(NetworkTarget::Single(client_id)),
            InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_id)),
            ControlledBy {
                owner: trigger.entity,
                lifetime: default(),
            },
        ))
        .id();

    info!(?player, ?client_id, slot, "spawned FPS player");
}

fn reset_fallen_players(
    mut players: Query<(
        &PlayerSlot,
        &mut Position,
        &mut Transform,
        &mut LinearVelocity,
        &mut CharacterControllerState,
    )>,
) {
    for (slot, mut position, mut transform, mut velocity, mut controller_state) in &mut players {
        if position.y >= -12.0 {
            continue;
        }
        let spawn = player_spawn_point(slot.0);
        position.0 = spawn;
        transform.translation = spawn;
        **velocity = Vec3::ZERO;
        *controller_state = CharacterControllerState::default();
    }
}
