//! Delivery of typed messages and events on the interpolation timeline.
//!
//! [`InterpolationPlugin`](crate::plugin::InterpolationPlugin) registers
//! [`InterpolationTimeline`](crate::timeline::InterpolationTimeline) as a
//! message-delivery timeline. Configure a dedicated channel with
//! [`ChannelSettings::with_timeline::<InterpolationTimeline>`](lightyear_transport::channel::builder::ChannelSettings::with_timeline).
//! The receiver then keeps typed messages and events sent on that channel
//! hidden until the interpolation timeline on that same connection entity
//! reaches the sender tick carried by the transport.

use bevy_app::{App, PreUpdate};
use bevy_ecs::schedule::IntoScheduleConfigs;
use lightyear_messages::plugin::{MessageSystems, register_message_timeline};

use crate::timeline::{InterpolationTimeline, TimelinePlugin};

pub(crate) fn configure_interpolated_messages(app: &mut App) {
    register_message_timeline::<InterpolationTimeline>(app);
    app.configure_sets(
        PreUpdate,
        MessageSystems::ReleaseTimeline.after(TimelinePlugin::advance_timeline),
    );
}
