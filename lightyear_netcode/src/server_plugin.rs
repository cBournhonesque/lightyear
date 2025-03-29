use crate::{NetcodeClient, NetcodeServer};
use alloc::rc::Rc;
use alloc::sync::Arc;
use bevy::ecs::entity::unique_slice::UniqueEntitySlice;
use bevy::prelude::*;
use bevy::tasks::futures_lite::StreamExt;
use lightyear_connection::id::ClientId;
use lightyear_connection::server::{ClientOf, Clients};
use lightyear_connection::ConnectionSet;
use lightyear_core::time::TimeManager;
use lightyear_link::{Link, LinkSet};
use lightyear_transport::plugin::TransportSet;
use lightyear_transport::prelude::Transport;
use tracing::error;

pub struct NetcodeServerPlugin;

#[derive(Component)]
pub struct Server {
    pub inner: NetcodeServer<()>
}


impl NetcodeServerPlugin {

    /// Takes packets from the Link, process them through the server,
    /// and buffer them back into the link to be sent by the IO
    fn send(
        mut server_query: Query<(&mut Server, &Clients)>,
        client_query: Query<(&mut Link, &ClientOf)>,
    ) {
        // TODO: we should be able to do ParIterMut if we can make the code understand
        //  that the transports/links are all mutually exclusive...
        //  Maybe some unsafe Cloneble wrapper around the client_query?
        //  Or maybe store the clients into a Local<Vec<(&mut Transport, &mut Link)>>? so that we can iterate faster through them?
        // we use Arc to tell the compiler that we know that the queries won't be used to access
        // the same clients (because each Link is uniquely associated with a single server)
        // This allow us to iterate in parallel over all servers
        let client_query = Arc::new(client_query);
        server_query.par_iter_mut().for_each(|(mut server, clients)| {
            // SAFETY: we know that each client is unique to a single server so we won't
            //  violate aliasing rules
            let mut client_query = unsafe { client_query.reborrow_unsafe() };

            // SAFETY: we know that the entities of a relationship are unique
            let unique_slice = unsafe { UniqueEntitySlice::from_slice_unchecked(clients.collection()) };
            client_query.iter_many_unique_mut(unique_slice).for_each(|(mut link, client_of)|  {
                 let ClientId::Netcode(client_id) = client_of.id else {
                    error!("Client {:?} is not a Netcode client", client_of.id);
                    return
                };

                link.send.drain(..).try_for_each(|payload| {
                    server.inner.buffer_send(payload, client_id)
                }).inspect_err(|e| {
                    error!("Error sending packet: {:?}", e);
                }).ok();

                // we don't want to short-circuit on error
                server.inner.send_buffered(link.send.as_mut()).inspect_err(|e| {
                    error!("Error sending packet: {:?}", e);
                }).ok();
            });
        })
    }

    /// Receive packets from the Link, process them through the server,
    /// then buffer them back into the Link
    fn receive(
        real_time: Res<Time<Real>>,
        mut server_query: Query<(&mut Server, &Clients)>,
        link_query: Query<&mut Link>,
    ) {
        let delta = real_time.delta();

        // we use Arc to tell the compiler that we know that the queries won't be used to access
        // the same clients (because each Link is uniquely associated with a single server)
        // This allow us to iterate in parallel over all servers
        let mut link_query = Arc::new(link_query);
        server_query.par_iter_mut().for_each(|(mut server, clients)| {

            // SAFETY: we know that each client is unique to a single server so we won't
            //  violate aliasing rules
            let mut link_query = unsafe { link_query.reborrow_unsafe() };

            // receive packets from the link and process them through the server
            server.inner.update_state(delta.as_secs_f64());

            // TODO: try to make this parallel!
            // SAFETY: we know that the list of client entities are unique because it is a Relationship
            let unique_slice = unsafe { UniqueEntitySlice::from_slice_unchecked(clients.collection()) };
            link_query.iter_many_unique_mut(unique_slice).for_each(|mut link| {
                 server.inner.receive(link.as_mut()).inspect_err(|e| {
                    error!("Error receiving packets: {:?}", e);
                }).ok();
            });

            // Buffer the packets received from the server into the Transport
            server.inner.recv().for_each(|(packet, client_id)| {
                // TODO: get the correct client_entity from the client_id
                //  or better yet, make the server return a Client Entity directly
                //  (the server maintains an internal mapping)
                let client_entity = Entity::PLACEHOLDER;
                let Ok(mut link) = link_query.get_mut(client_entity) else {
                    error!("Client {:?} not found", client_id);
                    return;
                };
                link.recv.push(packet);
            });
        })
    }
}


impl Plugin for NetcodeServerPlugin {
    fn build(&self, app: &mut App) {
        // TODO: should these be shared? or do we use Markers like in lightyear to distinguish between client and server?
        app.configure_sets(PreUpdate, (LinkSet::Receive, ConnectionSet::Receive, TransportSet::Receive).chain());
        app.configure_sets(PostUpdate, (TransportSet::Send, ConnectionSet::Send, LinkSet::Send));

        app.add_systems(PreUpdate, Self::receive.in_set(ConnectionSet::Receive));
        app.add_systems(PostUpdate, Self::send.in_set(ConnectionSet::Send));
    }
}