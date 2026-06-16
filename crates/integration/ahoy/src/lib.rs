//! # Lightyear Ahoy Integration
//!
//! Conservative integration helpers for driving `bevy_ahoy` character
//! controllers from Lightyear's predicted fixed tick.
//!
//! The crate parks Ahoy's automatic KCC schedule and exposes a manual stepper so
//! prediction, rollback replay, and server simulation can all consume exactly
//! one restored input tick before running exactly one KCC step.

pub mod plugin;
pub mod stepper;

#[cfg(feature = "native")]
pub mod native;

#[cfg(feature = "bei")]
pub mod bei;

pub mod prelude {
    #[cfg(all(feature = "bei", feature = "client"))]
    pub use crate::bei::write_bei_ahoy_user_commands;
    #[cfg(feature = "bei")]
    pub use crate::bei::{AhoyBeiInputPlugin, AhoyBeiLook};
    #[cfg(feature = "native")]
    pub use crate::native::{
        AhoyButtons, AhoyUserCommand, NativeAhoyInputPlugin, PreviousAhoyButtons,
        apply_user_command, clear_transient_input, step_native_ahoy_user_commands,
        tick_input_timers,
    };
    pub use crate::plugin::{LightyearAhoyKccSchedule, LightyearAhoyPlugin, LightyearAhoySystems};
    pub use crate::stepper::{AhoyStepParts, LightyearAhoyStepError, LightyearAhoyStepper};
}
