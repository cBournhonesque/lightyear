use bevy::prelude::*;
use bevy_ahoy::{
    CharacterController,
    prelude::{Climbdown, Crane, Crouch, Jump, Mantle, Movement, RotateCamera, SwimUp, Tac},
};
use bevy_enhanced_input::prelude::*;
use lightyear::{
    prelude::client::{InputDelayConfig, InputTimelineConfig},
    prelude::input::native::InputMarker,
    prelude::*,
};
use lightyear_ahoy::prelude::AhoyUserCommand;

use crate::{protocol::*, shared::*};

pub struct ExampleClientPlugin;

impl Plugin for ExampleClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, configure_input_delay);
        app.add_observer(handle_controlled_player);
        app.add_observer(handle_predicted_player);
    }
}

fn configure_input_delay(client: Single<Entity, With<Client>>, mut commands: Commands) {
    commands.entity(client.into_inner()).insert(
        InputTimelineConfig::default().with_input_delay(InputDelayConfig::no_input_delay()),
    );
}

fn handle_controlled_player(
    trigger: On<Add, Controlled>,
    mut commands: Commands,
    players: Query<
        Option<&ControlledBy>,
        (With<PlayerMarker>, Without<InputMarker<AhoyUserCommand>>),
    >,
    clients: Query<(), With<Client>>,
) {
    let entity = trigger.entity;
    let Ok(controlled_by) = players.get(entity) else {
        return;
    };
    if let Some(controlled_by) = controlled_by {
        if clients.get(controlled_by.owner).is_err() {
            return;
        }
    }

    commands.entity(entity).insert((
        PlayerInput,
        InputMarker::<AhoyUserCommand>::default(),
        actions!(PlayerInput[
            (
                Action::<Movement>::new(),
                DeadZone::default(),
                Bindings::spawn((Cardinal::wasd_keys(), Axial::left_stick()))
            ),
            (
                Action::<Jump>::new(),
                bindings![KeyCode::Space, GamepadButton::South],
            ),
            (
                Action::<SwimUp>::new(),
                bindings![KeyCode::Space, GamepadButton::South],
            ),
            (
                Action::<Crouch>::new(),
                bindings![KeyCode::ControlLeft, KeyCode::KeyC, GamepadButton::LeftTrigger2],
            ),
            (
                Action::<Tac>::new(),
                bindings![KeyCode::ShiftLeft, GamepadButton::LeftThumb],
            ),
            (
                Action::<Mantle>::new(),
                bindings![KeyCode::KeyE, GamepadButton::RightTrigger],
            ),
            (
                Action::<Crane>::new(),
                bindings![KeyCode::KeyQ, GamepadButton::LeftTrigger],
            ),
            (
                Action::<Climbdown>::new(),
                bindings![KeyCode::KeyZ, GamepadButton::DPadDown],
            ),
            (
                Action::<RotateCamera>::new(),
                Bindings::spawn((
                    Spawn((Binding::mouse_motion(), Scale::splat(0.07))),
                    Axial::right_stick().with((Scale::splat(4.0), DeadZone::default())),
                ))
            ),
        ]),
    ));
}

fn handle_predicted_player(
    trigger: On<Add, Predicted>,
    mut commands: Commands,
    players: Query<(), (With<PlayerMarker>, Without<CharacterController>)>,
) {
    let entity = trigger.entity;
    if players.get(entity).is_err() {
        return;
    }

    commands.entity(entity).insert(ahoy_player_bundle());
}
