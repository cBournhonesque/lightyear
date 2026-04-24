//! Shared schema for `lightyear_debug` tracing events.

/// Root tracing target used by the debug tooling.
pub const LIGHTYEAR_DEBUG_TARGET: &str = "lightyear_debug";

pub const LIGHTYEAR_DEBUG_TARGET_TIMELINE: &str = "lightyear_debug::timeline";
pub const LIGHTYEAR_DEBUG_TARGET_PREDICTION: &str = "lightyear_debug::prediction";
pub const LIGHTYEAR_DEBUG_TARGET_FRAME_INTERPOLATION: &str = "lightyear_debug::frame_interpolation";
pub const LIGHTYEAR_DEBUG_TARGET_INTERPOLATION: &str = "lightyear_debug::interpolation";
pub const LIGHTYEAR_DEBUG_TARGET_INPUT: &str = "lightyear_debug::input";
pub const LIGHTYEAR_DEBUG_TARGET_SYNC: &str = "lightyear_debug::sync";
pub const LIGHTYEAR_DEBUG_TARGET_MESSAGE: &str = "lightyear_debug::message";
pub const LIGHTYEAR_DEBUG_TARGET_ENTITY: &str = "lightyear_debug::entity";
pub const LIGHTYEAR_DEBUG_TARGET_TRANSPORT: &str = "lightyear_debug::transport";
pub const LIGHTYEAR_DEBUG_TARGET_COMPONENT: &str = "lightyear_debug::component";
pub const LIGHTYEAR_DEBUG_TARGET_MANUAL: &str = "lightyear_debug::manual";

/// Returns true when a tracing target belongs to Lightyear structured debugging.
#[inline]
pub fn is_lightyear_debug_target(target: &str) -> bool {
    target == LIGHTYEAR_DEBUG_TARGET
        || target
            .strip_prefix(LIGHTYEAR_DEBUG_TARGET)
            .is_some_and(|suffix| suffix.starts_with("::"))
}

/// High-level debug categories used as target suffixes and event fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DebugCategory {
    Timeline,
    Prediction,
    FrameInterpolation,
    Interpolation,
    Input,
    Sync,
    Message,
    Entity,
    Transport,
    Component,
    Manual,
}

impl DebugCategory {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Timeline => "timeline",
            Self::Prediction => "prediction",
            Self::FrameInterpolation => "frame_interpolation",
            Self::Interpolation => "interpolation",
            Self::Input => "input",
            Self::Sync => "sync",
            Self::Message => "message",
            Self::Entity => "entity",
            Self::Transport => "transport",
            Self::Component => "component",
            Self::Manual => "manual",
        }
    }

    pub const fn target(self) -> &'static str {
        match self {
            Self::Timeline => LIGHTYEAR_DEBUG_TARGET_TIMELINE,
            Self::Prediction => LIGHTYEAR_DEBUG_TARGET_PREDICTION,
            Self::FrameInterpolation => LIGHTYEAR_DEBUG_TARGET_FRAME_INTERPOLATION,
            Self::Interpolation => LIGHTYEAR_DEBUG_TARGET_INTERPOLATION,
            Self::Input => LIGHTYEAR_DEBUG_TARGET_INPUT,
            Self::Sync => LIGHTYEAR_DEBUG_TARGET_SYNC,
            Self::Message => LIGHTYEAR_DEBUG_TARGET_MESSAGE,
            Self::Entity => LIGHTYEAR_DEBUG_TARGET_ENTITY,
            Self::Transport => LIGHTYEAR_DEBUG_TARGET_TRANSPORT,
            Self::Component => LIGHTYEAR_DEBUG_TARGET_COMPONENT,
            Self::Manual => LIGHTYEAR_DEBUG_TARGET_MANUAL,
        }
    }
}

/// Logical sampling point within Bevy's schedules.
///
/// Keep new variants appended so consumers can map these names to stable indices
/// if they want an ordinal phase column at query time.
#[repr(u16)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DebugSamplePoint {
    Startup = 0,
    First = 1,
    PreUpdate = 2,
    StateTransition = 3,
    RunFixedMainLoop = 4,
    FixedFirst = 5,
    FixedPreUpdate = 6,
    FixedUpdateBeforePhysics = 7,
    FixedUpdate = 8,
    FixedUpdateAfterPhysics = 9,
    FixedPostUpdate = 10,
    FixedLast = 11,
    FixedLastAfterTransformPropagation = 12,
    Update = 13,
    SpawnScene = 14,
    PostUpdate = 15,
    Last = 16,
}

impl DebugSamplePoint {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Startup => "Startup",
            Self::First => "First",
            Self::PreUpdate => "PreUpdate",
            Self::StateTransition => "StateTransition",
            Self::RunFixedMainLoop => "RunFixedMainLoop",
            Self::FixedFirst => "FixedFirst",
            Self::FixedPreUpdate => "FixedPreUpdate",
            Self::FixedUpdateBeforePhysics => "FixedUpdateBeforePhysics",
            Self::FixedUpdate => "FixedUpdate",
            Self::FixedUpdateAfterPhysics => "FixedUpdateAfterPhysics",
            Self::FixedPostUpdate => "FixedPostUpdate",
            Self::FixedLast => "FixedLast",
            Self::FixedLastAfterTransformPropagation => "FixedLastAfterTransformPropagation",
            Self::Update => "Update",
            Self::SpawnScene => "SpawnScene",
            Self::PostUpdate => "PostUpdate",
            Self::Last => "Last",
        }
    }
}
