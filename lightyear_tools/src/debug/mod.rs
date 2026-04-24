//! Structured debug tracing helpers.

pub mod component;
pub mod metadata;
pub mod schema;

#[cfg(feature = "std")]
pub mod tracing_layer;

pub mod prelude {
    pub use crate::debug::component::{
        LightyearDebug, LightyearDebugAppExt, LightyearDebugComponentPlugin,
        LightyearDebugComponentRule, LightyearDebugComponentSamplerPlugin, log_component_value,
    };
    pub use crate::debug::manual_log;
    pub use crate::debug::metadata::{
        LightyearDebugFrame, LightyearDebugMetadata, LightyearDebugPlugin, LightyearDebugRole,
    };
    pub use crate::debug::schema::{
        DebugCategory, DebugSamplePoint, LIGHTYEAR_DEBUG_TARGET, LIGHTYEAR_DEBUG_TARGET_COMPONENT,
        LIGHTYEAR_DEBUG_TARGET_ENTITY, LIGHTYEAR_DEBUG_TARGET_FRAME_INTERPOLATION,
        LIGHTYEAR_DEBUG_TARGET_INPUT, LIGHTYEAR_DEBUG_TARGET_INTERPOLATION,
        LIGHTYEAR_DEBUG_TARGET_MANUAL, LIGHTYEAR_DEBUG_TARGET_MESSAGE,
        LIGHTYEAR_DEBUG_TARGET_PREDICTION, LIGHTYEAR_DEBUG_TARGET_SYNC,
        LIGHTYEAR_DEBUG_TARGET_TIMELINE, LIGHTYEAR_DEBUG_TARGET_TRANSPORT,
        is_lightyear_debug_target,
    };
    pub use crate::{lightyear_debug_component, lightyear_debug_event};

    #[cfg(feature = "std")]
    pub use crate::debug::tracing_layer::{
        LIGHTYEAR_DEBUG_FILE_ENV, LightyearDebugLayer, lightyear_debug_custom_layer,
        lightyear_debug_log_plugin, non_lightyear_debug_fmt_layer,
    };
}

#[doc(hidden)]
pub mod __private {
    pub use tracing;
}

/// Emit a user-authored row into the structured debug stream.
#[inline]
pub fn manual_log(message: &str) {
    crate::lightyear_debug_event!(
        crate::debug::schema::DebugCategory::Manual,
        crate::debug::schema::DebugSamplePoint::Update,
        "manual",
        "manual",
        message
    );
}

/// Emit a structured Lightyear debug event with common schedule/sample fields.
#[macro_export]
macro_rules! lightyear_debug_event {
    ($category:expr, $sample_point:expr, $schedule:expr, $kind:expr $(, $($field:tt)+)?) => {{
        let __sample_point = $sample_point;
        $crate::debug::__private::tracing::trace!(
            target: $category.target(),
            kind = $kind,
            sample_point = __sample_point.as_str(),
            schedule = $schedule
            $(, $($field)+)?
        );
    }};
}

/// Emit a structured component snapshot row.
#[macro_export]
macro_rules! lightyear_debug_component {
    ($entity:expr, $component:expr, $sample_point:expr, $schedule:expr) => {{
        $crate::debug::component::log_component_value(
            $entity,
            $component,
            $sample_point,
            $schedule,
            "component_value",
        );
    }};
    ($entity:expr, $component:expr, $sample_point:expr, $schedule:expr, $kind:expr) => {{
        $crate::debug::component::log_component_value(
            $entity,
            $component,
            $sample_point,
            $schedule,
            $kind,
        );
    }};
}
