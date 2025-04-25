//! Plugin to register and handle user inputs.

use crate::input_buffer::InputBuffer;
use crate::input_message::{ActionStateSequence, InputMessage};
use crate::InputChannel;
use bevy::app::{App, Plugin};
use bevy::ecs::entity::MapEntities;
use core::time::Duration;
use lightyear_connection::direction::NetworkDirection;
use lightyear_messages::prelude::AppMessageExt;
use lightyear_transport::prelude::{AppChannelExt, ChannelMode, ChannelSettings};

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
            // we send inputs every frame
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
            // .add_map_entities();

        app.register_required_components::<S::State, InputBuffer<S::State>>();
        app.register_required_components::<InputBuffer<S::State>, S::State>();
        app.register_required_components::<S::Marker, S::State>();
    }
}
