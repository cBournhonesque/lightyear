use crate::{Link, LinkSet, Linked, Linking, Unlink, Unlinked};
#[cfg(not(feature = "std"))]
use alloc::vec::Vec;
use bevy::app::{App, Plugin, PostUpdate, PreUpdate};
use bevy::ecs::component::HookContext;
use bevy::ecs::world::DeferredWorld;
use bevy::prelude::{Commands, Component, Entity, IntoScheduleConfigs, Query, Real, Reflect, RelationshipTarget, Res, Time, Trigger, With, Without};
use tracing::{info, trace};
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
    
    fn unlink(
        trigger: Trigger<Unlink>,
        mut query: Query<&ServerLink, Without<Unlinked>>,
        mut commands: Commands,
    ) {
        if let Ok(server_link) = query.get_mut(trigger.target()) {
            for link_of in server_link.collection() {
                if let Ok(mut c) = commands.get_entity(*link_of) {
                    c.trigger(Unlink {
                        reason: trigger.reason.clone()
                    });
                    c.despawn();
                }
            };
            commands.entity(trigger.target()).insert(Unlinked {
                reason: Some(trigger.reason.clone())
            });
        }
    }
}


#[derive(Component, Clone, Copy, PartialEq, Eq, Debug, Reflect)]
#[relationship(relationship_target = ServerLink)]
pub struct LinkOf {
    pub server: Entity
}

pub struct ServerLinkPlugin;
impl Plugin for ServerLinkPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(ServerLink::unlink);
    }
}
