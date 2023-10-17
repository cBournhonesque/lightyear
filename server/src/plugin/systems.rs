use bevy_ecs::prelude::{Res, ResMut};
use bevy_time::Time;
use tracing::trace;

use lightyear_shared::Protocol;

use crate::Server;

pub(crate) fn receive<P: Protocol>(time: Res<Time>, mut server: ResMut<Server<P>>) {
    trace!("Receive client packets");
    // update client state, send keep-alives, receive packets from io
    server.update(time.elapsed().as_secs_f64()).unwrap();
    // buffer packets into message managers
    server.recv_packets().unwrap();
}

pub(crate) fn send<P: Protocol>(mut server: ResMut<Server<P>>) {
    trace!("Send packets to clients");
    // send buffered packets to io
    server.send_packets().unwrap();
}

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
