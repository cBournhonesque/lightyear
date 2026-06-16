use bevy_ahoy::{
    AhoyKccPlugin, AhoySchedulePlugin, CharacterControllerState, CharacterLook,
    input::AccumulatedInput,
};
use bevy_app::prelude::*;
use bevy_ecs::intern::Interned;
use bevy_ecs::prelude::*;
use bevy_ecs::schedule::{IntoScheduleConfigs, ScheduleLabel};
use bevy_enhanced_input::EnhancedInputSystems;
use lightyear_prediction::prelude::PredictionAppRegistrationExt;

/// Parked schedule for Ahoy's automatic KCC.
///
/// `LightyearAhoyPlugin` installs Ahoy's KCC systems into this schedule, but
/// does not run the schedule. The conservative integration path instead calls
/// [`bevy_ahoy::CharacterControllerStepper::step_entity`] manually.
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct LightyearAhoyKccSchedule;

/// Public sets used by the integration.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LightyearAhoySystems {
    /// Convert restored input into Ahoy movement state.
    PrepareInput,
    /// Run one manual Ahoy KCC step for the current fixed tick.
    StepKcc,
    /// Mirror Ahoy's `Transform` output into Avian `Position`.
    SyncPosition,
}

/// Base plugin for conservative Ahoy integration.
///
/// This intentionally adds only Ahoy's KCC/schedule plugins. It does not add
/// `AhoyInputPlugin`, because conservative mode owns `AccumulatedInput`
/// clearing/timer ticking inside the replayed fixed path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LightyearAhoyPlugin {
    /// Add Ahoy's KCC plugin in a parked schedule.
    pub add_parked_ahoy_kcc: bool,
    /// Register reusable Ahoy movement state for Lightyear rollback.
    pub register_rollback: bool,
}

impl Default for LightyearAhoyPlugin {
    fn default() -> Self {
        Self {
            add_parked_ahoy_kcc: true,
            register_rollback: true,
        }
    }
}

impl Plugin for LightyearAhoyPlugin {
    fn build(&self, app: &mut App) {
        if self.add_parked_ahoy_kcc {
            let schedule: Interned<dyn ScheduleLabel> = LightyearAhoyKccSchedule.intern();
            app.add_plugins((AhoySchedulePlugin { schedule }, AhoyKccPlugin { schedule }));
        }

        if self.register_rollback {
            app.add_rollback::<CharacterControllerState>();
            app.add_rollback::<AccumulatedInput>();
            app.add_rollback::<CharacterLook>();
        }

        app.configure_sets(
            FixedPreUpdate,
            (
                LightyearAhoySystems::PrepareInput,
                LightyearAhoySystems::StepKcc,
            )
                .chain()
                .after(EnhancedInputSystems::Apply),
        );

        #[cfg(feature = "client")]
        app.configure_sets(
            FixedPreUpdate,
            LightyearAhoySystems::PrepareInput
                .after(lightyear_inputs::client::InputSystems::BufferClientInputs),
        );

        #[cfg(feature = "server")]
        app.configure_sets(
            FixedPreUpdate,
            LightyearAhoySystems::PrepareInput
                .after(lightyear_inputs::server::InputSystems::UpdateActionState),
        );
    }
}
