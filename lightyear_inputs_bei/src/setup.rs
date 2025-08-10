//! For BEI, option 1:
//! - Server spawns Context entity
//! - Client spawns ActionOf entities with Action<A> components + Bindings
//!   (which inserts all other required components, such as ActionSettings, ActionFns)
//!   ActionFns is needed to trigger events
//!
//! Then:
//! - Client sends an initial message to the server that contains
//!   - for a given context, a vec of (Entity, kind) where kind is the type of InputAction
//!
//! Alternative:
//! - client inserts Replicate on the ActionOf entities, but only ActionOf component is replicated from client -> server (which should be ok since other components won't be in the protocol)
//!
//! Option 2:
//! - shared system to spawn context + actions on both client and server, and we need to perform entity mapping.

use bevy_app::App;
use bevy_ecs::prelude::*;
use bevy_enhanced_input::prelude::*;
use lightyear_replication::components::Replicated;
use lightyear_replication::prelude::{AppComponentExt, ComponentReplicationConfig, Replicate};
use lightyear_serde::SerializationError;
use lightyear_serde::registry::SerializeFns;
use lightyear_serde::writer::Writer;

pub struct InputRegistryPlugin;

impl InputRegistryPlugin {
    /// When an Action<A> component is added to an entity (usually on the client),
    /// we add Replicate to it so that the action entity is also created on the server.
    fn add_action_replicate<A: InputAction>(
        trigger: Trigger<OnAdd, Action<A>>,
        // we don't want to add Replicate on entities that were already replicated
        query: Query<(), Without<Replicated>>,
        mut commands: Commands,
    ) {
        if query.get(trigger.target()).is_ok() {
            commands
                .entity(trigger.target())
                .insert(Replicate::to_server());
        }
    }

    // we don't care about the actual data in Action<A>, so nothing to serialize
    fn serialize_action<A: InputAction>(
        _: &Action<A>,
        _: &mut Writer,
    ) -> core::result::Result<(), SerializationError> {
        Ok(())
    }
    fn deserialize_action<A: InputAction>(
        _: &mut lightyear_serde::reader::Reader,
    ) -> core::result::Result<Action<A>, SerializationError> {
        Ok(Action::<A>::default())
    }
}

pub trait InputRegistryExt {
    /// Registers a new input action type and returns its kind.
    fn register_input_action<A: InputAction>(self) -> Self;
}

impl InputRegistryExt for &mut App {
    fn register_input_action<A: InputAction>(self) -> Self {
        // Register the Action<A> component so that it can be also added on the server
        self.register_component_custom_serde::<Action<A>>(SerializeFns::<Action<A>> {
            serialize: InputRegistryPlugin::serialize_action::<A>,
            deserialize: InputRegistryPlugin::deserialize_action::<A>,
        })
        .with_replication_config(ComponentReplicationConfig {
            replicate_once: true,
            disable: false,
            delta_compression: false,
        });

        // Add observer to add Replicate when Action<A> is added, so that the Action entities are created on the server as well
        self.add_observer(InputRegistryPlugin::add_action_replicate::<A>);

        self
    }
}
