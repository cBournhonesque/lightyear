use bevy::prelude::{Mut, ResMut, Time, World};
use tracing::trace;

use lightyear_shared::Protocol;

use crate::Client;

pub(crate) fn receive<P: Protocol>(world: &mut World) {
    trace!("Receive server packets");
    world.resource_scope(|world, mut client: Mut<Client<P>>| {
        let time = world.get_resource::<Time>().unwrap();

        // update client state, send keep-alives, receive packets from io
        client.update(time.elapsed().as_secs_f64()).unwrap();
        // buffer packets into message managers
        client.recv_packets().unwrap();
        // receive packets from message managers
        let events = client.receive(world);
        if !events.is_empty() {
            dbg!(events.spawns);
            // panic!();
        }
    });
}

pub(crate) fn send<P: Protocol>(mut client: ResMut<Client<P>>) {
    trace!("Send packets to server");
    // send buffered packets to io
    client.send_packets().unwrap();
}
