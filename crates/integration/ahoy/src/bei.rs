#[cfg(feature = "client")]
use crate::native::AhoyButtons;
use crate::native::{AhoyUserCommand, NativeAhoyInputPlugin};
#[cfg(feature = "client")]
use crate::plugin::LightyearAhoySystems;
#[cfg(feature = "client")]
use bevy_ahoy::prelude::{
    Climbdown, Crane, Crouch, Jump, Mantle, Movement, RotateCamera, SwimUp, Tac,
};
use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
#[cfg(feature = "client")]
use bevy_ecs::relationship::Relationship;
#[cfg(feature = "client")]
use bevy_enhanced_input::{
    EnhancedInputPlugin, EnhancedInputSystems,
    context::InputContextAppExt,
    prelude::{Action, ActionOf, InputAction},
};
use bevy_math::Vec2;
use bevy_reflect::Reflect;
use core::{f32::consts::TAU, marker::PhantomData};
#[cfg(feature = "client")]
use lightyear_inputs::client::InputSystems;
use lightyear_inputs::config::InputConfig;
#[cfg(feature = "client")]
use lightyear_inputs_native::prelude::{ActionState, InputMarker};

/// Local look accumulator for the BEI bridge.
///
/// `RotateCamera` is interpreted the same way Ahoy's own camera integration
/// interprets it: the action value is a degree delta, usually produced from
/// mouse motion through a BEI `Scale` modifier.
#[derive(Component, Clone, Copy, Debug, PartialEq, Reflect)]
#[reflect(Component)]
pub struct AhoyBeiLook {
    pub yaw: f32,
    pub pitch: f32,
    pub pitch_min: f32,
    pub pitch_max: f32,
    pub rotate_scale: f32,
}

impl Default for AhoyBeiLook {
    fn default() -> Self {
        Self {
            yaw: 0.0,
            pitch: 0.0,
            pitch_min: -TAU / 4.0 + 0.01,
            pitch_max: TAU / 4.0 - 0.01,
            rotate_scale: 1.0,
        }
    }
}

impl AhoyBeiLook {
    pub fn apply_rotate_camera(&mut self, rotate: Vec2) {
        let delta = -rotate * self.rotate_scale;
        self.yaw += delta.x.to_radians();
        self.pitch = (self.pitch + delta.y.to_radians()).clamp(self.pitch_min, self.pitch_max);
    }

    pub fn command_look(&self) -> Vec2 {
        Vec2::new(self.yaw, self.pitch)
    }
}

/// Samples Ahoy BEI actions into the hidden native `AhoyUserCommand`.
///
/// This keeps the public game-facing input model in BEI while reusing
/// Lightyear's native input buffering, networking, and rollback for the compact
/// command that drives Ahoy KCC simulation.
pub struct AhoyBeiInputPlugin<C> {
    pub config: InputConfig<AhoyUserCommand>,
    pub add_input_context: bool,
    marker: PhantomData<C>,
}

impl<C> Default for AhoyBeiInputPlugin<C> {
    fn default() -> Self {
        Self {
            config: InputConfig::default(),
            add_input_context: true,
            marker: PhantomData,
        }
    }
}

impl<C> AhoyBeiInputPlugin<C> {
    pub fn new(config: InputConfig<AhoyUserCommand>) -> Self {
        Self {
            config,
            ..Self::default()
        }
    }

    pub fn without_input_context(mut self) -> Self {
        self.add_input_context = false;
        self
    }
}

impl<C: Component> Plugin for AhoyBeiInputPlugin<C> {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<NativeAhoyInputPlugin>() {
            app.add_plugins(NativeAhoyInputPlugin {
                config: self.config,
            });
        }

        #[cfg(feature = "client")]
        {
            if !app.is_plugin_added::<EnhancedInputPlugin>() {
                app.add_plugins(EnhancedInputPlugin);
            }
            if self.add_input_context {
                app.add_input_context_to::<FixedPreUpdate, C>();
            }

            app.register_type::<AhoyBeiLook>();
            app.register_required_components::<InputMarker<AhoyUserCommand>, AhoyBeiLook>();
            app.configure_sets(
                FixedPreUpdate,
                InputSystems::WriteClientInputs.after(EnhancedInputSystems::Apply),
            );
            app.add_systems(
                FixedPreUpdate,
                write_bei_ahoy_user_commands::<C>
                    .in_set(InputSystems::WriteClientInputs)
                    .before(LightyearAhoySystems::PrepareInput),
            );
        }
    }
}

#[cfg(feature = "client")]
pub fn write_bei_ahoy_user_commands<C: Component>(
    mut commands: Query<
        (Entity, &mut ActionState<AhoyUserCommand>, &mut AhoyBeiLook),
        With<InputMarker<AhoyUserCommand>>,
    >,
    movement: Query<(&ActionOf<C>, &Action<Movement>)>,
    rotate_camera: Query<(&ActionOf<C>, &Action<RotateCamera>)>,
    jump: Query<(&ActionOf<C>, &Action<Jump>)>,
    crouch: Query<(&ActionOf<C>, &Action<Crouch>)>,
    tac: Query<(&ActionOf<C>, &Action<Tac>)>,
    mantle: Query<(&ActionOf<C>, &Action<Mantle>)>,
    crane: Query<(&ActionOf<C>, &Action<Crane>)>,
    climbdown: Query<(&ActionOf<C>, &Action<Climbdown>)>,
    swim_up: Query<(&ActionOf<C>, &Action<SwimUp>)>,
) {
    for (entity, mut command, mut look) in &mut commands {
        if let Some(rotate_camera) = action_value(entity, &rotate_camera) {
            look.apply_rotate_camera(rotate_camera);
        }

        command.0 = AhoyUserCommand {
            movement: action_value(entity, &movement).unwrap_or_default(),
            look: look.command_look(),
            buttons: AhoyButtons {
                jump: action_value(entity, &jump).unwrap_or(false),
                crouch: action_value(entity, &crouch).unwrap_or(false),
                tac: action_value(entity, &tac).unwrap_or(false),
                mantle: action_value(entity, &mantle).unwrap_or(false),
                crane: action_value(entity, &crane).unwrap_or(false),
                climbdown: action_value(entity, &climbdown).unwrap_or(false),
                swim_up: action_value(entity, &swim_up).unwrap_or(false),
            },
        };
    }
}

#[cfg(feature = "client")]
fn action_value<C, A>(
    context: Entity,
    actions: &Query<(&ActionOf<C>, &Action<A>)>,
) -> Option<A::Output>
where
    C: Component,
    A: InputAction,
    A::Output: Copy,
{
    actions
        .iter()
        .find_map(|(action_of, action)| (action_of.get() == context).then_some(**action))
}
