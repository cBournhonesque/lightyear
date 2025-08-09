#![no_std]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

pub mod server;

use alloc::format;

use aeronet_io::connection::{Disconnect, Disconnected, LocalAddr, PeerAddr};
use aeronet_io::server::{Close, Server};
use aeronet_io::{IoSet, Session, SessionEndpoint};
use bevy_app::{App, Plugin, PostUpdate, PreUpdate};
use bevy_ecs::{
    component::Component,
    entity::Entity,
    observer::Trigger,
    query::{Has, With},
    relationship::Relationship,
    schedule::IntoScheduleConfigs,
    system::{Commands, Query},
    world::OnAdd,
};
use bevy_reflect::Reflect;
use lightyear_link::{
    Link, LinkPlugin, LinkReceiveSet, LinkSet, Linked, Linking, Unlink, Unlinked,
};
use tracing::trace;

/// The lightyear Link entity
#[derive(Component, Reflect)]
#[relationship_target(relationship = AeronetLinkOf, linked_spawn)]
pub struct AeronetLink(#[relationship] Entity);

/// The Aeronet Session entity
#[derive(Component, Reflect)]
#[relationship(relationship_target = AeronetLink)]
pub struct AeronetLinkOf(pub Entity);

pub struct AeronetPlugin;

impl AeronetPlugin {
    /// If LocalAddr is added on the AeronetLink entity, it will be copied to the AeronetLinkOf entity.
    fn on_local_addr_added(
        trigger: Trigger<OnAdd, (LocalAddr, AeronetLinkOf)>,
        query: Query<(&AeronetLinkOf, &LocalAddr)>,
        mut commands: Commands,
    ) {
        if let Ok((aeronet_link, local_addr)) = query.get(trigger.target())
            && let Ok(mut c) = commands.get_entity(aeronet_link.0)
        {
            trace!(
                "LocalAddr added on AeronetLink {:?}. Adding on Link entity {:?}",
                trigger.target(),
                aeronet_link.0
            );
            c.insert(LocalAddr(local_addr.0));
        }
    }

    /// If PeerAddr is added on the AeronetLink entity, it will be copied to the AeronetLinkOf entity.
    fn on_peer_addr_added(
        trigger: Trigger<OnAdd, (PeerAddr, AeronetLinkOf)>,
        query: Query<(&AeronetLinkOf, &PeerAddr)>,
        mut commands: Commands,
    ) {
        if let Ok((aeronet_link, peer_addr)) = query.get(trigger.target())
            && let Ok(mut c) = commands.get_entity(aeronet_link.0)
        {
            trace!(
                "PeerAddr added on AeronetLink {:?}. Adding on Link entity {:?}",
                trigger.target(),
                aeronet_link.0
            );
            c.insert(PeerAddr(peer_addr.0));
        }
    }

    fn on_connecting(
        trigger: Trigger<OnAdd, (SessionEndpoint, AeronetLinkOf)>,
        query: Query<&AeronetLinkOf, With<SessionEndpoint>>,
        mut commands: Commands,
    ) {
        if let Ok(aeronet_link) = query.get(trigger.target())
            && let Ok(mut c) = commands.get_entity(aeronet_link.0)
        {
            trace!(
                "SessionEndpoint added on AeronetLink {:?}. Adding Linking on Link entity {:?}",
                trigger.target(),
                aeronet_link.0
            );
            c.insert(Linking);
        }
    }

    fn on_connected(
        trigger: Trigger<OnAdd, (Session, AeronetLinkOf)>,
        query: Query<&AeronetLinkOf, With<Session>>,
        mut commands: Commands,
    ) {
        if let Ok(aeronet_link) = query.get(trigger.target())
            && let Ok(mut c) = commands.get_entity(aeronet_link.0)
        {
            trace!(
                "Session added on AeronetLink {:?}. Adding Linked on Link entity {:?}",
                trigger.target(),
                aeronet_link.0
            );
            c.insert(Linked);
        }
    }

    fn on_disconnected(
        trigger: Trigger<Disconnected>,
        query: Query<&AeronetLinkOf>,
        mut commands: Commands,
    ) {
        if let Ok(aeronet_io) = query.get(trigger.target())
            && let Ok(mut c) = commands.get_entity(aeronet_io.0)
        {
            let reason = match &*trigger {
                Disconnected::ByUser(reason) => {
                    format!("Disconnected by user: {reason}")
                }
                Disconnected::ByPeer(reason) => {
                    format!("Disconnected by remote: {reason}")
                }
                Disconnected::ByError(err) => {
                    format!("Disconnected due to error: {err:?}")
                }
            };
            trace!(
                "Disconnected (reason: {reason:?}) triggered added on AeronetLink {:?}. Adding Unlinked on Link entity {:?}",
                trigger.target(),
                aeronet_io.0
            );
            // we try insert, because the LinkOf entity might have been despawned already
            c.try_insert(Unlinked { reason });
        }
    }

    fn unlink(
        mut trigger: Trigger<Unlink>,
        query: Query<&AeronetLink>,
        aeronet_query: Query<Has<Server>>,
        mut commands: Commands,
    ) {
        if let Ok(aeronet_link) = query.get(trigger.target())
            // get the aeronet session entity
            && let Ok(is_server) = aeronet_query.get(aeronet_link.0)
        {
            let reason = core::mem::take(&mut trigger.reason);
            trace!(
                "Unlink triggered on Link entity {:?} (reason: {reason:?}). Closing/Disconnecting AeronetLink entity {:?}",
                trigger.target(),
                aeronet_link.0
            );
            if is_server {
                commands.entity(aeronet_link.0).trigger(Close::new(reason));
            } else {
                commands
                    .entity(aeronet_link.0)
                    .trigger(Disconnect::new(reason));
            }
        }
    }

    fn receive(
        mut session_query: Query<(&mut Session, &AeronetLinkOf)>,
        mut link_query: Query<&mut Link, With<Linked>>,
    ) {
        session_query.iter_mut().for_each(|(mut session, parent)| {
            if let Ok(mut link) = link_query.get_mut(parent.get()) {
                trace!("Received {:?} packets", session.recv.len());
                session.recv.drain(..).for_each(|recv| {
                    #[cfg(feature = "test_utils")]
                    link.recv
                        .push(recv.payload, lightyear_core::time::Instant::now());
                    #[cfg(not(feature = "test_utils"))]
                    link.recv.push(recv.payload, recv.recv_at);
                });
            }
        });
    }

    fn send(
        mut session_query: Query<(&mut Session, &AeronetLinkOf)>,
        mut link_query: Query<&mut Link, With<Linked>>,
    ) {
        session_query.iter_mut().for_each(|(mut session, parent)| {
            if let Ok(mut link) = link_query.get_mut(parent.get()) {
                trace!("Sending {:?} packet", link.send.len());
                link.send.drain().for_each(|payload| {
                    session.send.push(payload);
                });
            }
        });
    }
}

impl Plugin for AeronetPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<LinkPlugin>() {
            app.add_plugins(LinkPlugin);
        }

        app.register_type::<AeronetLinkOf>()
            .register_type::<AeronetLink>();

        app.add_observer(Self::on_local_addr_added);
        app.add_observer(Self::on_peer_addr_added);
        app.add_observer(Self::on_connecting);
        app.add_observer(Self::on_connected);
        app.add_observer(Self::on_disconnected);
        app.add_observer(Self::unlink);

        app.configure_sets(PreUpdate, LinkSet::Receive.after(IoSet::Poll));
        app.configure_sets(PostUpdate, LinkSet::Send.before(IoSet::Flush));
        app.add_systems(
            PreUpdate,
            Self::receive.in_set(LinkReceiveSet::BufferToLink),
        );
        app.add_systems(PostUpdate, Self::send.in_set(LinkSet::Send));
    }
}
