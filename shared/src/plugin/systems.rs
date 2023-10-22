use bevy_app::{App, PostUpdate};
use bevy_ecs::change_detection::Ref;
use bevy_ecs::prelude::{Added, Component, DetectChanges, Entity, IntoSystemConfigs, ResMut};
use bevy_ecs::system::Query;
use tracing::debug;

use crate::replication::{Replicate, ReplicationSend};
use crate::{Protocol, ReplicationSet};

pub fn send_entity_spawn<P: Protocol, R: ReplicationSend<P>>(
    // try doing entity spawn whenever replicate gets added
    query: Query<(Entity, &Replicate), Added<Replicate>>,
    // query: Query<(Entity, &Replicate)>,
    mut sender: ResMut<R>,
) {
    query.iter().for_each(|(entity, replicate)| {
        sender.entity_spawn(entity, vec![], replicate).unwrap();
    })
}

pub fn send_component_update<C: Component + Clone, P: Protocol, R: ReplicationSend<P>>(
    query: Query<(Entity, Ref<C>, &Replicate)>,
    mut sender: ResMut<R>,
) where
    <P as Protocol>::Components: From<C>,
{
    query.iter().for_each(|(entity, component, replicate)| {
        // // send an component_insert for components that were newly added
        // if component.is_added() {
        //     sender
        //         .component_insert(entity, component.clone().into(), replicate)
        //         .unwrap();
        // }
        // only update components that were not newly added ?
        if component.is_changed() && !component.is_added() {
            sender
                .entity_update_single_component(entity, component.clone().into(), replicate)
                .unwrap();
        }
    })
}

pub fn add_replication_send_systems<C: Component + Clone, P: Protocol, R: ReplicationSend<P>>(
    app: &mut App,
) where
    <P as Protocol>::Components: From<C>,
{
    app.add_systems(
        PostUpdate,
        (send_entity_spawn::<P, R>, send_component_update::<C, P, R>).in_set(ReplicationSet::Send),
    );
}
