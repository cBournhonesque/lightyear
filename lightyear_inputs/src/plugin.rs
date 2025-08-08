//! Plugin to register and handle user inputs.

use crate::InputChannel;
use crate::input_buffer::InputBuffer;
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
            // sending every frame is ok because:
            // - we want the clients to send inputs as fast as possible
            // - the server might have a very high frame rate but it's only
            //   rebroadcast inputs when it receives them
            send_frequency: Duration::default(),
            // we always want to include the inputs in the packet
            priority: f32::INFINITY,
        })
        // bidirectional in case of rebroadcasting inputs
        .add_direction(NetworkDirection::Bidirectional);

        app.add_message::<InputMessage<S>>()
            // add entity mapping for:
            // - server receiving pre-predicted entities
            // - client receiving other players' inputs
            // - input itself containing entities
            .add_map_entities()
            .add_direction(NetworkDirection::Bidirectional);

        S::register_required_components(app);
    }
}
