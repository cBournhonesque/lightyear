extern crate alloc;

use alloc::format;

pub mod server;

use aeronet_io::connection::{Disconnect, Disconnected, LocalAddr, PeerAddr};
use aeronet_io::server::{Close, Server};
use aeronet_io::{IoSet, Session, SessionEndpoint};
use bevy_app::{App, Plugin, PostUpdate, PreUpdate};
use bevy_ecs::query::{Has, With};
use bevy_ecs::relationship::Relationship;
use bevy_ecs::schedule::IntoScheduleConfigs;
use bevy_ecs::{
    component::Component,
    entity::Entity,
    observer::Trigger,
    system::{Commands, Query},
    world::OnAdd,
};
use bevy_reflect::Reflect;
use lightyear_link::{Link, LinkPlugin, LinkSet, Linked, Linking, Unlink, Unlinked};
use tracing::trace;

#[derive(Component, Reflect)]
#[relationship_target(relationship = AeronetLinkOf, linked_spawn)]
pub struct AeronetLink(#[relationship] Entity);

#[derive(Component, Reflect)]
#[relationship(relationship_target = AeronetLink)]
pub struct AeronetLinkOf(pub Entity);

pub struct AeronetPlugin;

impl AeronetPlugin {
    fn on_local_addr_added(
        trigger: Trigger<OnAdd, (LocalAddr, AeronetLinkOf)>,
        query: Query<(&AeronetLinkOf, &LocalAddr)>,
        mut commands: Commands,
    ) {
        if let Ok((aeronet_link, local_addr)) = query.get(trigger.target()) {
            if let Ok(mut c) = commands.get_entity(aeronet_link.0) {
                c.insert(LocalAddr(local_addr.0));
            }
        }
    }

    fn on_peer_addr_added(
        trigger: Trigger<OnAdd, (PeerAddr, AeronetLinkOf)>,
        query: Query<(&AeronetLinkOf, &PeerAddr)>,
        mut commands: Commands,
    ) {
        if let Ok((aeronet_link, peer_addr)) = query.get(trigger.target()) {
            if let Ok(mut c) = commands.get_entity(aeronet_link.0) {
                c.insert(PeerAddr(peer_addr.0));
            }
        }
    }

    fn on_connecting(
        trigger: Trigger<OnAdd, (SessionEndpoint, AeronetLinkOf)>,
        query: Query<&AeronetLinkOf>,
        mut commands: Commands,
    ) {
        if let Ok(aeronet_link) = query.get(trigger.target()) {
            if let Ok(mut c) = commands.get_entity(aeronet_link.0) {
                c.insert(Linking);
            }
        }
    }

    fn on_connected(
        trigger: Trigger<OnAdd, (Session, AeronetLinkOf)>,
        query: Query<&AeronetLinkOf>,
        mut commands: Commands,
    ) {
        if let Ok(aeronet_link) = query.get(trigger.target()) {
            if let Ok(mut c) = commands.get_entity(aeronet_link.0) {
                c.insert(Linked);
            }
        }
    }

    fn on_disconnected(
        trigger: Trigger<Disconnected>,
        query: Query<&AeronetLinkOf>,
        mut commands: Commands,
    ) {
        if let Ok(aeronet_io) = query.get(trigger.target()) {
            if let Ok(mut c) = commands.get_entity(aeronet_io.0) {
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
                c.insert(Unlinked { reason });
            }
        }
    }

    fn unlink(
        mut trigger: Trigger<Unlink>,
        query: Query<&AeronetLink>,
        aeronet_query: Query<Has<Server>>,
        mut commands: Commands,
    ) {
        if let Ok(aeronet_link) = query.get(trigger.target()) {
            // get the aeronet session entity
            if let Ok(is_server) = aeronet_query.get(aeronet_link.0) {
                let reason = core::mem::take(&mut trigger.reason);
                if is_server {
                    commands.entity(aeronet_link.0).trigger(Close::new(reason));
                } else {
                    commands
                        .entity(aeronet_link.0)
                        .trigger(Disconnect::new(reason));
                }
            }
        }
    }

    fn receive(
        mut session_query: Query<(&mut Session, &AeronetLinkOf)>,
        mut link_query: Query<&mut Link, With<Linked>>,
    ) {
        session_query.iter_mut().for_each(|(mut session, parent)| {
            if let Ok(mut link) = link_query.get_mut(parent.get()) {
                session.recv.drain(..).for_each(|recv| {
                    trace!("Received packet");
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
                link.send.drain().for_each(|payload| {
                    trace!("Send packet");
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
        app.add_systems(PreUpdate, Self::receive.in_set(LinkSet::Receive));
        app.add_systems(PostUpdate, Self::send.in_set(LinkSet::Send));
    }
}
