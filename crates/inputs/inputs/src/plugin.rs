//! Plugin to register and handle user inputs.

use crate::InputChannel;
use crate::input_message::{ActionStateSequence, InputMessage};
use bevy_app::{App, Plugin};
use bevy_ecs::entity::MapEntities;
use core::time::Duration;
use lightyear_connection::direction::NetworkDirection;
use lightyear_messages::prelude::AppMessageExt;
use lightyear_transport::prelude::{AppChannelExt, ChannelMode, ChannelSettings};

#[doc(hidden)]
pub struct InputPlugin<S> {
    _marker: core::marker::PhantomData<S>,
}

impl<S> Default for InputPlugin<S> {
    fn default() -> Self {
        Self {
            _marker: core::marker::PhantomData,
        }
    }
}

impl<S: ActionStateSequence + MapEntities> Plugin for InputPlugin<S> {
    fn build(&self, app: &mut App) {
        app.add_channel::<InputChannel>(ChannelSettings {
            mode: ChannelMode::UnorderedUnreliable,
            // Input channel should flush as soon as it has data in order to
            // minimize latency between input capture and the peer receiving
            // it. Data rate is controlled by the producer
            // (`prepare_input_message` which is gated on
            // `InputConfig::send_interval`), not here. Raising this above zero
            // adds extra latency on top of `send_interval`. Raising it above
            // `send_interval` makes the producer run multiple times before the
            // buffer is flushed, inserting duplicate data into the buffer.
            send_frequency: Duration::default(),
            // we always want to include the inputs in the packet
            priority: f32::INFINITY,
            // Inputs produced on a later frame supersede a locally unsent input message.
            retry_unsent_messages: false,
        })
        // bidirectional in case of rebroadcasting inputs
        .add_direction(NetworkDirection::Bidirectional);

        app.register_message::<InputMessage<S>>()
            // add entity mapping for:
            // - server receiving pre-predicted entities
            // - client receiving other players' inputs
            // - input itself containing entities
            .add_map_entities()
            .add_direction(NetworkDirection::Bidirectional);

        S::register_required_components(app);
    }
}
