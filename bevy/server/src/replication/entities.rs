use bevy_ecs::prelude::{Added, Entity, Query, ResMut};
use lightyear_bevy_shared::replication::entities::ReplicationMessage;
use lightyear_server::Server;
use lightyear_shared::replication::Replicate;
use lightyear_shared::{MessageContainer, Protocol};

// 1. server-spawned entities are sent to the client via reliable-channel
// 2. wait for ack to be sure that entity has been spawned on the client?
fn replicate_entity_spawn<P: Protocol>(
    mut server: ResMut<Server<P>>,
    query: Query<(Entity, &Replicate), Added<Replicate>>,
) {
    // TODO: distinguish between new entity or just replicate got added.
    //  Maybe by adding an extra component the first time the entity gets created? or a flag in the Replicate component?

    for (entity, replicate) in query.iter() {
        server.entity_spawn(entity, replicate);
    }
}
