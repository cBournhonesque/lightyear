use crate::{Link, LinkPlugin, LinkSet, Linked, Linking, Unlink, Unlinked};
#[cfg(not(feature = "std"))]
use alloc::vec::Vec;
use bevy::ecs::component::HookContext;
use bevy::ecs::world::DeferredWorld;
use bevy::prelude::*;
use lightyear_core::prelude::LocalTimeline;
use tracing::{info, trace};
// TODO: should we also have a LinkId (remote addr/etc.) that uniquely identifies the link?

#[derive(Component, Default, Debug, PartialEq, Eq, Reflect)]
#[component(on_add = Server::on_add)]
#[relationship_target(relationship = LinkOf, linked_spawn)]
pub struct Server {
    links: Vec<Entity>,
}

impl Server {
    fn on_add(mut world: DeferredWorld, context: HookContext) {
        let entity_ref = world.entity(context.entity);
        if !entity_ref.contains::<Unlinked>()
            && !entity_ref.contains::<Linked>()
             && !entity_ref.contains::<Linking>() {
            trace!("Inserting Unlinked because ServerLink was added");
            world.commands().entity(context.entity)
                .insert(Unlinked { reason: String::new()});
        };
    }

    fn unlinked(
        trigger: Trigger<OnAdd, Unlinked>,
        mut query: Query<(&Server, &Unlinked)>,
        mut commands: Commands,
    ) {
        if let Ok((server_link, unlinked)) = query.get_mut(trigger.target()) {
            for link_of in server_link.collection() {
                if let Ok(mut c) = commands.get_entity(*link_of) {
                    // cannot simply insert Unlinked because then we wouldn't close aeronet sessions...
                    c.trigger(Unlink {
                        reason: unlinked.reason.clone()
                    });
                    c.despawn();
                }
            };
        }
    }
}


#[derive(Component, Clone, Copy, PartialEq, Eq, Debug, Reflect)]
#[relationship(relationship_target = Server)]
pub struct LinkOf {
    pub server: Entity
}

pub struct ServerLinkPlugin;
impl Plugin for ServerLinkPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<LinkPlugin>() {
            app.add_plugins(LinkPlugin);
        }
        app.register_required_components::<Server, LocalTimeline>();
        app.add_observer(Server::unlinked);
    }
}
