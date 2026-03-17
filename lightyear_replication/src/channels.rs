#![allow(unused_imports)]

use alloc::vec::Vec;
use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use lightyear_connection::direction::NetworkDirection;
use lightyear_transport::channel::builder::ChannelMode;
use lightyear_transport::channel::builder::ChannelSettings;
use lightyear_transport::channel::builder::ReliableSettings;
use lightyear_transport::channel::registry::{ChannelId, ChannelKind};
use lightyear_transport::prelude::{AppChannelExt, ChannelRegistry};

/// Marker type for replicon's ServerChannel::Updates (Ordered Reliable, ServerToClient)
pub struct RepliconUpdatesChannel;

/// Marker type for replicon's ServerChannel::Mutations (Unreliable, ServerToClient)
pub struct RepliconMutationsChannel;

/// Marker type for replicon's ClientChannel::MutationAcks (Ordered Reliable, ClientToServer)
pub struct RepliconMutationAcksChannel;

/// Maps between replicon's usize channel indices and lightyear's ChannelKind/ChannelId.
///
/// Replicon has two separate channel namespaces (server and client), both starting from index 0.
/// Server sends on server_channels, receives on client_channels.
/// Client does the reverse.
#[derive(Resource)]
pub struct RepliconChannelMap {
    /// replicon server channel index -> lightyear (ChannelKind, ChannelId)
    pub server_channels: Vec<(ChannelKind, ChannelId)>,
    /// replicon client channel index -> lightyear (ChannelKind, ChannelId)
    pub client_channels: Vec<(ChannelKind, ChannelId)>,
}

/// Plugin that registers replicon's core channels as lightyear transport channels
/// and builds the `RepliconChannelMap` resource.
pub struct RepliconChannelRegistrationPlugin;

impl Plugin for RepliconChannelRegistrationPlugin {
    fn build(&self, app: &mut App) {
        // ServerChannel::Updates - Ordered Reliable, Bidirectional
        // (both client and server can replicate entities)
        // High priority: entity actions (spawn/despawn/insert/remove) should be sent ASAP
        app.add_channel::<RepliconUpdatesChannel>(ChannelSettings {
            mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
            priority: 10.0,
            ..Default::default()
        })
        .add_direction(NetworkDirection::Bidirectional);

        // ServerChannel::Mutations - Unreliable, Bidirectional
        // Low priority: component mutations can be delayed if bandwidth is limited
        app.add_channel::<RepliconMutationsChannel>(ChannelSettings {
            mode: ChannelMode::UnorderedUnreliable,
            priority: 1.0,
            ..Default::default()
        })
        .add_direction(NetworkDirection::Bidirectional);

        // ClientChannel::MutationAcks - Ordered Reliable, Bidirectional
        // High priority: acks should be sent ASAP to avoid unnecessary resends
        app.add_channel::<RepliconMutationAcksChannel>(ChannelSettings {
            mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
            priority: 10.0,
            ..Default::default()
        })
        .add_direction(NetworkDirection::Bidirectional);
    }

    fn finish(&self, app: &mut App) {
        let registry = app.world().resource::<ChannelRegistry>();

        let updates_kind = ChannelKind::of::<RepliconUpdatesChannel>();
        let mutations_kind = ChannelKind::of::<RepliconMutationsChannel>();
        let mutation_acks_kind = ChannelKind::of::<RepliconMutationAcksChannel>();

        let updates_id = *registry.get_net_from_kind(&updates_kind).unwrap();
        let mutations_id = *registry.get_net_from_kind(&mutations_kind).unwrap();
        let mutation_acks_id = *registry.get_net_from_kind(&mutation_acks_kind).unwrap();

        // server_channels: index 0 = Updates, index 1 = Mutations
        // (matches ServerChannel::Updates = 0, ServerChannel::Mutations = 1)
        let server_channels =
            alloc::vec![(updates_kind, updates_id), (mutations_kind, mutations_id),];

        // client_channels: index 0 = MutationAcks
        // (matches ClientChannel::MutationAcks = 0)
        let client_channels = alloc::vec![(mutation_acks_kind, mutation_acks_id),];

        app.insert_resource(RepliconChannelMap {
            server_channels,
            client_channels,
        });
    }
}
