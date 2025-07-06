use crate::{LinkPlugin, Linked, Linking, Unlink, Unlinked};
use alloc::{format, string::String, vec::Vec};
use bevy_app::{App, Plugin};
use bevy_ecs::{
    component::{Component, HookContext},
    entity::Entity,
    observer::Trigger,
    query::{With, Without},
    relationship::{
        Relationship, RelationshipHookMode, RelationshipSourceCollection, RelationshipTarget,
    },
    system::{Commands, Query},
    world::{DeferredWorld, OnAdd, OnInsert},
};
use bevy_reflect::Reflect;
use lightyear_core::prelude::LocalTimeline;
use tracing::{trace, warn};
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
            && !entity_ref.contains::<Linking>()
        {
            trace!("Inserting Unlinked because Server was added");
            world.commands().entity(context.entity).insert(Unlinked {
                reason: String::new(),
            });
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
                        reason: unlinked.reason.clone(),
                    });
                    c.despawn();
                }
            }
        }
    }
}

// We add our own relationship hooks instead of deriving relationship
//  because we don't want to despawn Server if there are no more LinkOfs.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug, Reflect)]
#[component(on_insert = LinkOf::on_insert_hook)]
#[component(on_replace = LinkOf::on_replace)]
pub struct LinkOf {
    pub server: Entity,
}


impl Relationship for LinkOf {
    type RelationshipTarget = Server;
    #[inline(always)]
    fn get(&self) -> Entity {
        self.server
    }
    #[inline]
    fn from(entity: Entity) -> Self {
        Self { server: entity}
    }
}

impl LinkOf {
    fn on_insert_hook(
        mut world: DeferredWorld,
        HookContext {
            entity,
            caller,
            relationship_hook_mode,
            ..
        }: HookContext,
    ) {
        match relationship_hook_mode {
            RelationshipHookMode::Run => {}
            RelationshipHookMode::Skip => return,
            RelationshipHookMode::RunIfNotLinked => return,
        }
        let target_entity = world.entity(entity).get::<Self>().unwrap().get();
        if target_entity == entity {
            warn!(
                "{}The {}({target_entity:?}) relationship on entity {entity:?} points to itself. The invalid {} relationship has been removed.",
                caller
                    .map(|location| format!("{location}: "))
                    .unwrap_or_default(),
                core::any::type_name::<Self>(),
                core::any::type_name::<Self>()
            );
            world.commands().entity(entity).remove::<Self>();
            return;
        }
        if let Ok(mut target_entity_mut) = world.get_entity_mut(target_entity) {
            if let Some(mut relationship_target) = target_entity_mut.get_mut::<Server>() {
                relationship_target.collection_mut_risky().add(entity);
            } else {
                let mut target = <Server as RelationshipTarget>::with_capacity(1);
                target.collection_mut_risky().add(entity);
                world.commands().entity(target_entity).insert(target);
            }
        } else {
            warn!(
                "{}The {}({target_entity:?}) relationship on entity {entity:?} relates to an entity that does not exist. The invalid {} relationship has been removed.",
                caller
                    .map(|location| format!("{location}: "))
                    .unwrap_or_default(),
                core::any::type_name::<Self>(),
                core::any::type_name::<Self>()
            );
            world.commands().entity(entity).remove::<Self>();
        }
    }

    fn on_replace(
        mut world: DeferredWorld,
        HookContext {
            entity,
            relationship_hook_mode,
            ..
        }: HookContext,
    ) {
        match relationship_hook_mode {
            RelationshipHookMode::Run => {}
            RelationshipHookMode::Skip => return,
            RelationshipHookMode::RunIfNotLinked => {
                if <Server as RelationshipTarget>::LINKED_SPAWN {
                    return;
                }
            }
        }
        let target_entity = world.entity(entity).get::<Self>().unwrap().get();
        if let Ok(mut target_entity_mut) = world.get_entity_mut(target_entity) {
            if let Some(mut relationship_target) = target_entity_mut.get_mut::<Server>() {
                RelationshipSourceCollection::remove(
                    relationship_target.collection_mut_risky(),
                    entity,
                );
            }
        }
    }

    pub(crate) fn on_insert(
        trigger: Trigger<OnInsert, (LinkOf, LocalTimeline)>,
        server: Query<&LocalTimeline, (Without<LinkOf>, With<Server>)>,
        mut query: Query<(&mut LocalTimeline, &LinkOf)>,
    ) {
        if let Ok((mut timeline, link_of)) = query.get_mut(trigger.target()) {
            if let Ok(server_timeline) = server.get(link_of.get()) {
                *timeline = server_timeline.clone();
            }
        }
    }
}

pub struct ServerLinkPlugin;
impl Plugin for ServerLinkPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<LinkPlugin>() {
            app.add_plugins(LinkPlugin);
        }
        app.register_required_components::<Server, LocalTimeline>();
        app.add_observer(Server::unlinked);
        app.add_observer(LinkOf::on_insert);
    }
}
