use crate::NetcodeClient;
use bevy::prelude::*;
use core::net::SocketAddr;
use lightyear_connection::ConnectionSet;
use lightyear_core::time::TimeManager;
use lightyear_link::{Link, LinkSet};
use lightyear_packet::prelude::{Transport, TransportSet};
use tracing::error;

pub struct NetcodeClientPlugin;

#[derive(Component)]
pub struct Client {
    pub inner: NetcodeClient<()>,
}

// TODO: set the remote_addr on the Link upon connection?

impl NetcodeClientPlugin {
    /// Takes packets from the Transport, process them through the client,
    /// and buffer them into the link to be sent by the IO
    fn send(
        mut query: Query<(&mut Transport, &mut Link, &mut Client)>,
    ) {
        query.par_iter_mut().for_each(|(mut transport, mut link, mut client)| {
            transport.send.drain(..).for_each(|payload| {
                // we don't want to short-circuit on error
                client.inner.send(payload, link.as_mut()).inspect_err(|e| {
                    error!("Error sending packet: {:?}", e);
                }).ok();
            })
        })
    }

    /// Receive packets from the Link, and process them through the client,
    /// then buffer them into the Transport
    fn receive(
        real_time: Res<Time<Real>>,
        mut query: Query<(&mut Transport, &mut Link, &mut Client)>,
    ) {
        let delta = real_time.delta();
        query.par_iter_mut().for_each(|(mut transport, mut link, mut client)| {
            // Buffer the packets received from the link into the Connection
            // don't short-circuit on error
            client.inner.try_update(delta.as_secs_f64(), link.as_mut())
                .inspect_err(|e| {
                    error!("Error receiving packet: {:?}", e);
                })
                .ok();

            // Buffer the packets received from the Connection into the Transport
            while let Some(packet) = client.inner.recv() {
                transport.recv.push(packet);
            }
        })
    }
}


impl Plugin for NetcodeClientPlugin {
    fn build(&self, app: &mut App) {
        app.configure_sets(PreUpdate, (LinkSet::Receive, ConnectionSet::Receive,  TransportSet::Receive).chain());
        app.configure_sets(PostUpdate, (TransportSet::Send, ConnectionSet::Send, LinkSet::Send));

        app.add_systems(PreUpdate, Self::receive.in_set(ConnectionSet::Receive));
        app.add_systems(PostUpdate, Self::send.in_set(ConnectionSet::Send));
    }
}