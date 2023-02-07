//! # Naia Shared
//! Common functionality shared between naia-server & naia-client crates.

#![deny(trivial_numeric_casts, unstable_features, unused_import_braces)]


cfg_if! {
    if #[cfg(all(target_arch = "wasm32", feature = "wbindgen", feature = "mquad"))]
    {
        // Use both protocols...
        compile_error!("wasm target for 'naia_shared' crate requires either the 'wbindgen' OR 'mquad' feature to be enabled, you must pick one.");
    }
    else if #[cfg(all(target_arch = "wasm32", not(feature = "wbindgen"), not(feature = "mquad")))]
    {
        // Use no protocols...
        compile_error!("wasm target for 'naia_shared' crate requires either the 'wbindgen' or 'mquad' feature to be enabled, you must pick one.");
    }
}

pub use naia_socket_shared::{Instant, LinkConditionerConfig, Random, SocketConfig};

pub use lightyear_derive::*;
pub use lightyear_serde as serde;

mod backends;
mod component;
mod connection;
mod entity;
mod messages;

mod bigmap;
mod constants;
mod key_generator;
mod protocol;
mod types;
mod wrapping_number;

pub use backends::{Timer, Timestamp};
pub use component::{
    component_update::ComponentUpdate,
    diff_mask::DiffMask,
    property::Property,
    property_mutate::{PropertyMutate, PropertyMutator},
    replica_ref::{
        ReplicaDynMut, ReplicaDynMutTrait, ReplicaDynMutWrapper, ReplicaDynRef, ReplicaDynRefTrait,
        ReplicaDynRefWrapper, ReplicaMutTrait, ReplicaMutWrapper, ReplicaRefTrait,
        ReplicaRefWrapper,
    },
    replicate::{Components, Replicate, ReplicateSafe},
};
pub use connection::{
    ack_manager::AckManager,
    bandwidth_monitor::BandwidthMonitor,
    base_connection::BaseConnection,
    compression_config::{CompressionConfig, CompressionMode},
    connection_config::ConnectionConfig,
    decoder::Decoder,
    encoder::Encoder,
    packet_notifiable::PacketNotifiable,
    packet_type::PacketType,
    ping_config::PingConfig,
    ping_manager::{PingIndex, PingManager},
    standard_header::StandardHeader,
};
pub use entity::{
    entity_action::EntityAction,
    entity_action_receiver::EntityActionReceiver,
    entity_action_type::EntityActionType,
    entity_property::{
        EntityConverter, EntityDoesNotExistError, EntityProperty,
        FakeEntityConverter, NetEntityConverter,
    },
    net_entity::NetEntity,
};
pub use messages::{
    channel_config::{
        Channel, ChannelDirection, ChannelMode, Channels, ReliableSettings, TickBufferSettings,
    },
    message::{Message, Messages},
    message_channel::{ChannelReader, ChannelReceiver, ChannelSender, ChannelWriter},
    message_manager::MessageManager,
    message_receivable::MessageReceivable,
    named::Named,
    ordered_reliable_receiver::OrderedReliableReceiver,
    protocol_io::ProtocolIo,
    reliable_sender::ReliableSender,
    unordered_reliable_receiver::UnorderedReliableReceiver,
};

pub use bigmap::{BigMap, BigMapKey};
pub use constants::MESSAGE_HISTORY_SIZE;
pub use key_generator::KeyGenerator;
pub use protocol::{Plugin, Protocol, ProtocolBuilder};
pub use types::{
    ChannelId, ComponentId, HostType, MessageId, MessageIndex, PacketIndex, ShortMessageIndex, Tick,
};
pub use wrapping_number::{sequence_greater_than, sequence_less_than, wrapping_diff};
