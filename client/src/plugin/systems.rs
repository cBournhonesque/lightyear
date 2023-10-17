use bevy_ecs::prelude::{Res, ResMut};
use bevy_time::Time;
use tracing::trace;

use lightyear_shared::Protocol;

use crate::Client;

pub(crate) fn receive<P: Protocol>(time: Res<Time>, mut client: ResMut<Client<P>>) {
    trace!("Receive server packets");
    // update client state, send keep-alives, receive packets from io
    client.update(time.elapsed().as_secs_f64()).unwrap();
    // buffer packets into message managers
    client.recv_packets().unwrap();
}

pub(crate) fn send<P: Protocol>(mut client: ResMut<Client<P>>) {
    trace!("Send packets to server");
    // send buffered packets to io
    client.send_packets().unwrap();
}
