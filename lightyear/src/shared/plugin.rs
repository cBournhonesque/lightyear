//! Bevy [`bevy::prelude::Plugin`] used by both the server and the client
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use crate::_reexport::ShouldBeInterpolated;
use replication::hierarchy::HierarchySyncPlugin;

use crate::client::config::ClientConfig;
use crate::inputs::native::input_buffer::InputData;
use crate::prelude::*;
use crate::shared::config::SharedConfig;
use crate::shared::replication;
use crate::shared::replication::components::{
    PerComponentReplicationMetadata, Replicate, ReplicationGroupIdBuilder,
};
use crate::shared::tick_manager::TickManagerPlugin;

pub struct SharedPlugin<P: Protocol> {
    pub config: SharedConfig,
    pub _marker: std::marker::PhantomData<P>,
}

impl<P: Protocol> Default for SharedPlugin<P> {
    fn default() -> Self {
        Self {
            config: SharedConfig::default(),
            _marker: std::marker::PhantomData,
        }
    }
}

/// You can use this as a SystemParam to identify whether you're running on the client or the server
#[derive(SystemParam)]
pub struct NetworkIdentity<'w, 's> {
    config: Option<Res<'w, ClientConfig>>,
    _marker: std::marker::PhantomData<&'s ()>,
}

impl<'w, 's> NetworkIdentity<'w, 's> {
    pub fn is_client(&self) -> bool {
        self.config.is_some()
    }
    pub fn is_server(&self) -> bool {
        self.config.is_none()
    }
}

impl<P: Protocol> Plugin for SharedPlugin<P> {
    fn build(&self, app: &mut App) {
        // REFLECT
        app.register_type::<Replicate<P>>();
        app.register_type::<PerComponentReplicationMetadata>();
        app.register_type::<ReplicationGroupIdBuilder>();
        app.register_type::<ReplicationGroup>();
        app.register_type::<ReplicationMode>();
        app.register_type::<NetworkTarget>();
        app.register_type::<ShouldBeInterpolated>();
        app.register_type::<ShouldBePredicted>();
        app.register_type::<ClientMetadata>();
        app.register_type::<ChannelBuilder>();
        app.register_type::<ChannelDirection>();
        app.register_type::<ChannelMode>();
        app.register_type::<ChannelSettings>();
        app.register_type::<ReliableSettings>();
        app.register_type::<PreSpawnedPlayerObject>();
        app.register_type::<InputData<P::Input>>();
        // input
        app.register_type::<crate::inputs::native::InputMessage<P::Input>>();
        #[cfg(feature = "leafwing")]
        {
            app.register_type::<crate::inputs::leafwing::input_buffer::InputTarget>();
            app.register_type::<crate::inputs::leafwing::InputMessage<P::LeafwingInput1>>();
            app.register_type::<crate::inputs::leafwing::InputMessage<P::LeafwingInput2>>();
            app.register_type::<crate::inputs::leafwing::input_buffer::ActionDiff<P::LeafwingInput1>>();
            app.register_type::<crate::inputs::leafwing::input_buffer::ActionDiff<P::LeafwingInput2>>();
        }

        // RESOURCES
        // NOTE: this tick duration must be the same as any previous existing fixed timesteps
        app.insert_resource(Time::<Fixed>::from_seconds(
            self.config.tick.tick_duration.as_secs_f64(),
        ));

        // PLUGINS
        app.add_plugins(HierarchySyncPlugin::<P>::default());
        app.add_plugins(TickManagerPlugin {
            config: self.config.tick.clone(),
        });
    }
}
