//! Plugin to register and handle user inputs.

use crate::input_buffer::InputBuffer;
use crate::input_message::{ActionStateSequence, InputMessage};
use crate::InputChannel;
use bevy_app::{App, Plugin};
use bevy_ecs::entity::MapEntities;
use lightyear_connection::direction::NetworkDirection;
use lightyear_core::tick::TickDuration;
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
        let tick_duration = app.world().resource::<TickDuration>();
        app.add_channel::<InputChannel>(ChannelSettings {
            mode: ChannelMode::UnorderedUnreliable,
            // we send inputs roughly every tick
            send_frequency: tick_duration.0,
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

        app.register_required_components::<S::State, InputBuffer<S::Snapshot>>();
        app.register_required_components::<InputBuffer<S::Snapshot>, S::State>();
        app.try_register_required_components::<S::Marker, S::State>()
            .ok();
    }
}
