use crate::client::config::ClientConfig;
use crate::prelude::{ChannelDirection, Message};
use crate::protocol::message::registry::AppMessageExt;
use crate::protocol::SerializeFns;
use crate::server::config::ServerConfig;
use crate::shared::replication::resources::DespawnResource;
use bevy::app::App;
use bevy::prelude::Resource;
use serde::de::DeserializeOwned;
use serde::Serialize;

fn register_resource_send<R: Resource + Message>(app: &mut App, direction: ChannelDirection) {
    let is_client = app.world().get_resource::<ClientConfig>().is_some();
    let is_server = app.world().get_resource::<ServerConfig>().is_some();
    match direction {
        ChannelDirection::ClientToServer => {
            if is_client {
                crate::shared::replication::resources::send::add_resource_send_systems::<
                    R,
                    crate::prelude::client::ConnectionManager,
                >(app);
            }
            if is_server {
                crate::shared::replication::resources::receive::add_resource_receive_systems::<
                    R,
                    crate::prelude::server::ConnectionManager,
                >(app, false);
            }
        }
        ChannelDirection::ServerToClient => {
            if is_server {
                crate::shared::replication::resources::send::add_resource_send_systems::<
                    R,
                    crate::prelude::server::ConnectionManager,
                >(app);
            }
            if is_client {
                crate::shared::replication::resources::receive::add_resource_receive_systems::<
                    R,
                    crate::prelude::client::ConnectionManager,
                >(app, false);
            }
        }
        ChannelDirection::Bidirectional => {
            if is_server {
                crate::shared::replication::resources::send::add_resource_send_systems::<
                    R,
                    crate::prelude::server::ConnectionManager,
                >(app);
                crate::shared::replication::resources::receive::add_resource_receive_systems::<
                    R,
                    crate::prelude::server::ConnectionManager,
                >(app, true);
            }
            if is_client {
                crate::shared::replication::resources::send::add_resource_send_systems::<
                    R,
                    crate::prelude::client::ConnectionManager,
                >(app);
                crate::shared::replication::resources::receive::add_resource_receive_systems::<
                    R,
                    crate::prelude::client::ConnectionManager,
                >(app, true);
            }
            // register_resource_send::<R>(app, ChannelDirection::ClientToServer);
            // register_resource_send::<R>(app, ChannelDirection::ServerToClient);
        }
    }
}

pub trait AppResourceExt {
    /// Registers the resource in the Registry
    /// This resource can now be sent over the network.
    fn register_resource<R: Resource + Message + Serialize + DeserializeOwned>(
        &mut self,
        direction: ChannelDirection,
    );

    /// Registers the resource in the Registry
    ///
    /// This resource can now be sent over the network.
    /// You need to provide your own [`SerializeFns`] for this message
    fn register_resource_custom_serde<R: Resource + Message>(
        &mut self,
        direction: ChannelDirection,
        serialize_fns: SerializeFns<R>,
    );
}

impl AppResourceExt for App {
    /// Register a resource to be automatically replicated over the network
    fn register_resource<R: Resource + Message + Serialize + DeserializeOwned>(
        &mut self,
        direction: ChannelDirection,
    ) {
        self.register_message::<R>(direction);
        self.register_message::<DespawnResource<R>>(direction);
        register_resource_send::<R>(self, direction)
    }

    /// Register a resource to be automatically replicated over the network
    fn register_resource_custom_serde<R: Resource + Message>(
        &mut self,
        direction: ChannelDirection,
        serialize_fns: SerializeFns<R>,
    ) {
        self.register_message_custom_serde::<R>(direction, serialize_fns);
        self.register_message::<DespawnResource<R>>(direction);
        register_resource_send::<R>(self, direction)
    }
}
