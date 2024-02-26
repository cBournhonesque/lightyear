use bevy::ecs::system::{Command, EntityCommands};
use bevy::prelude::{Entity, World};

use crate::_reexport::ReplicationSend;
use crate::prelude::Protocol;
use crate::shared::replication::components::Replicate;

pub struct RemoveReplicate;

fn remove_replicate<P: Protocol, R: ReplicationSend<P>>(entity: Entity, world: &mut World) {
    let mut sender = world.resource_mut::<R>();
    // remove the entity from the cache of entities that are being replicated
    sender.get_mut_replicate_component_cache().remove(&entity);
    // remove the replicate component
    if let Some(mut entity) = world.get_entity_mut(entity) {
        entity.remove::<Replicate<P>>();
    }
}

pub trait RemoveReplicateCommandsExt<P: Protocol, R: ReplicationSend<P>> {
    /// Remove the replicate component from the entity.
    /// This also makes sure that if you despawn the entity right after, the despawn won't be replicated.
    ///
    /// This can be useful when you want to despawn an entity on the server, but you don't want the despawn to be replicated
    /// immediately to clients (for example because clients are playing a despawn animation)/
    fn remove_replicate(&mut self);
}
impl<P: Protocol, R: ReplicationSend<P>> RemoveReplicateCommandsExt<P, R> for EntityCommands<'_> {
    fn remove_replicate(&mut self) {
        self.add(remove_replicate::<P, R>);
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn test_despawn() {}
}
