//! Delivery of typed messages and events on the interpolation timeline.
//!
//! [`InterpolationPlugin`](crate::plugin::InterpolationPlugin) registers
//! [`InterpolationTimeline`](crate::timeline::InterpolationTimeline) as a
//! message-delivery timeline. Configure a dedicated channel with
//! [`ChannelSettings::with_timeline::<InterpolationTimeline>`](lightyear_transport::channel::builder::ChannelSettings::with_timeline).
//! Register messages for that timeline and read them from
//! `MessageReceiver<M, InterpolationTimeline>`. That receiver owns the pending
//! buffer until the interpolation timeline on the same connection entity
//! reaches the sender tick carried by the transport. Events use an equivalent
//! internal timeline buffer and are triggered when ready.

use bevy_app::{App, PreUpdate};
use bevy_ecs::schedule::IntoScheduleConfigs;
use lightyear_messages::plugin::{MessageSystems, register_message_timeline};
use lightyear_messages::receive::BufferedMessageTimeline;

use crate::timeline::{InterpolationTimeline, TimelinePlugin};

impl BufferedMessageTimeline for InterpolationTimeline {}

pub(crate) fn configure_interpolated_messages(app: &mut App) {
    register_message_timeline::<InterpolationTimeline>(app);
    app.configure_sets(
        PreUpdate,
        MessageSystems::ReleaseTimeline.after(TimelinePlugin::advance_timeline),
    );
}
