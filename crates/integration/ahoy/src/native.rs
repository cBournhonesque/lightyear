use crate::{
    plugin::LightyearAhoySystems,
    stepper::{LightyearAhoyStepError, LightyearAhoyStepper},
};
use bevy_ahoy::{CharacterController, CharacterLook, input::AccumulatedInput};
use bevy_app::prelude::*;
use bevy_ecs::entity::{EntityMapper, MapEntities};
use bevy_ecs::prelude::*;
use bevy_math::Vec2;
use bevy_reflect::Reflect;
use bevy_time::Stopwatch;
use core::time::Duration;
use lightyear_inputs::config::InputConfig;
use lightyear_inputs_native::prelude::{ActionState, InputPlugin};
use serde::{Deserialize, Serialize};

/// Button inputs consumed by Ahoy movement.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Reflect, Debug, Default)]
pub struct AhoyButtons {
    pub jump: bool,
    pub crouch: bool,
    pub tac: bool,
    pub mantle: bool,
    pub crane: bool,
    pub climbdown: bool,
    pub swim_up: bool,
}

/// Compact native Lightyear command for conservative Ahoy movement.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Reflect, Debug, Default)]
pub struct AhoyUserCommand {
    pub movement: Vec2,
    /// Absolute yaw/pitch for this fixed tick, in radians.
    pub look: Vec2,
    pub buttons: AhoyButtons,
}

impl MapEntities for AhoyUserCommand {
    fn map_entities<M: EntityMapper>(&mut self, _entity_mapper: &mut M) {}
}

/// Last button state consumed by the conservative native adapter.
#[derive(Component, Serialize, Deserialize, Clone, Copy, PartialEq, Reflect, Debug, Default)]
#[reflect(Component)]
pub struct PreviousAhoyButtons(pub AhoyButtons);

/// Native command adapter plugin.
///
/// This plugin registers Lightyear's native input plugin for
/// [`AhoyUserCommand`] and steps all Ahoy character controllers that have an
/// `ActionState<AhoyUserCommand>`.
pub struct NativeAhoyInputPlugin {
    pub config: InputConfig<AhoyUserCommand>,
}

impl Default for NativeAhoyInputPlugin {
    fn default() -> Self {
        Self {
            config: InputConfig::default(),
        }
    }
}

impl Plugin for NativeAhoyInputPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<AhoyButtons>();
        app.register_type::<AhoyUserCommand>();
        app.register_type::<PreviousAhoyButtons>();
        app.add_plugins(InputPlugin::<AhoyUserCommand> {
            config: self.config,
        });
        app.add_systems(
            FixedPreUpdate,
            step_native_ahoy_user_commands.in_set(LightyearAhoySystems::StepKcc),
        );
    }
}

/// Step Ahoy character controllers from native Lightyear commands.
pub fn step_native_ahoy_user_commands(
    mut commands: Commands,
    mut stepper: LightyearAhoyStepper,
    mut query: Query<
        (
            Entity,
            &ActionState<AhoyUserCommand>,
            Option<&mut PreviousAhoyButtons>,
        ),
        With<CharacterController>,
    >,
) {
    for (entity, command, previous_buttons) in &mut query {
        let previous = previous_buttons
            .as_deref()
            .map_or(AhoyButtons::default(), |previous| previous.0);
        let command = command.0;
        if let Err(error) = step_ahoy_user_command(&mut stepper, entity, command, previous) {
            tracing::warn!(
                ?entity,
                ?error,
                "failed to step Ahoy character controller from native input"
            );
        }

        if let Some(mut previous_buttons) = previous_buttons {
            previous_buttons.0 = command.buttons;
        } else {
            commands
                .entity(entity)
                .insert(PreviousAhoyButtons(command.buttons));
        }
    }
}

/// Step one entity from an [`AhoyUserCommand`].
pub fn step_ahoy_user_command(
    stepper: &mut LightyearAhoyStepper,
    entity: Entity,
    command: AhoyUserCommand,
    previous_buttons: AhoyButtons,
) -> Result<(), LightyearAhoyStepError> {
    stepper.step_entity_with_input(entity, |fixed_delta, input, look| {
        tick_input_timers(input, fixed_delta);
        clear_transient_input(input);
        apply_user_command(input, look, command, previous_buttons);
    })
}

/// Advance Ahoy's input timers by one fixed tick.
pub fn tick_input_timers(input: &mut AccumulatedInput, delta: Duration) {
    if let Some(timer) = input.jumped.as_mut() {
        timer.tick(delta);
    }
    if let Some(timer) = input.tac.as_mut() {
        timer.tick(delta);
    }
    if let Some(timer) = input.craned.as_mut() {
        timer.tick(delta);
    }
    if let Some(timer) = input.mantled.as_mut() {
        timer.tick(delta);
    }
    if let Some(timer) = input.climbdown.as_mut() {
        timer.tick(delta);
    }
}

/// Clear transient Ahoy input fields before applying the current tick command.
pub fn clear_transient_input(input: &mut AccumulatedInput) {
    input.last_movement = None;
    input.swim_up = false;
    input.crouched = false;
}

/// Convert a compact command into Ahoy movement state.
pub fn apply_user_command(
    input: &mut AccumulatedInput,
    look: &mut CharacterLook,
    command: AhoyUserCommand,
    previous_buttons: AhoyButtons,
) {
    input.last_movement = Some(command.movement.clamp_length_max(1.0));
    input.swim_up = command.buttons.swim_up;
    input.crouched = command.buttons.crouch;

    if command.buttons.jump && !previous_buttons.jump {
        input.jumped = Some(Stopwatch::new());
    }
    if command.buttons.tac && !previous_buttons.tac {
        input.tac = Some(Stopwatch::new());
    }
    if command.buttons.crane && !previous_buttons.crane {
        input.craned = Some(Stopwatch::new());
    }
    if command.buttons.mantle && !previous_buttons.mantle {
        input.mantled = Some(Stopwatch::new());
    }
    if command.buttons.climbdown && !previous_buttons.climbdown {
        input.climbdown = Some(Stopwatch::new());
    }

    look.yaw = command.look.x;
    look.pitch = command.look.y.clamp(-1.5, 1.5);
}
