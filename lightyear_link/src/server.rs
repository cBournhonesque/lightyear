use crate::{Linked, Linking, Unlinked};
#[cfg(not(feature = "std"))]
use alloc::vec::Vec;
use bevy::ecs::component::HookContext;
use bevy::ecs::world::DeferredWorld;
use bevy::prelude::{Component, Entity, Reflect};
use tracing::trace;
// TODO: should we also have a LinkId (remote addr/etc.) that uniquely identifies the link?

#[derive(Component, Default, Debug, PartialEq, Eq, Reflect)]
#[component(on_add = ServerLink::on_add)]
#[relationship_target(relationship = LinkOf, linked_spawn)]
pub struct ServerLink {
    links: Vec<Entity>,
}

impl ServerLink {
    fn on_add(mut world: DeferredWorld, context: HookContext) {
        let entity_ref = world.entity(context.entity);
        if !entity_ref.contains::<Unlinked>()
            && !entity_ref.contains::<Linked>()
             && !entity_ref.contains::<Linking>() {
            trace!("Inserting Unlinked because ServerLink was added");
            world.commands().entity(context.entity)
                .insert(Unlinked { reason: None});
        };
    }
}


#[derive(Component, Clone, Copy, PartialEq, Eq, Debug, Reflect)]
#[relationship(relationship_target = ServerLink)]
pub struct LinkOf {
    pub server: Entity
}

