use crate::Server;
use bevy_ecs::entity::Entity;
use bevy_ecs::prelude::{Query, ResMut};
use bevy_ecs::query::Added;
use lightyear_shared::replication::Replicate;
use lightyear_shared::Protocol;

// fn replicate_entity_spawn<P: Protocol>(
//     mut server: ResMut<Server<P>>,
//     query: Query<(Entity, &Replicate), Added<Replicate>>,
// ) {
//     // TODO: distinguish between new entity or just replicate got added.
//     //  Maybe by adding an extra component the first time the entity gets created? or a flag in the Replicate component?
//
//     for (entity, replicate) in query.iter() {
//         server.entity_spawn(entity, replicate);
//     }
// }
