//! # Lightyear Crossbeam
//!
//! This crate provides a transport layer for Lightyear that uses `crossbeam-channel`.
//! It's primarily intended for local testing or scenarios where in-process message passing
//! is desired, simulating a network connection without actual network I/O.
//!
//! It defines `CrossbeamIo` for channel-based communication and `CrossbeamPlugin`
//! to integrate this transport into a Bevy application.
#![no_std]

extern crate alloc;

use aeronet_io::connection::{LocalAddr, PeerAddr};
use bevy_app::{App, Plugin, PostUpdate, PreUpdate};
use bevy_ecs::query::QueryData;
use bevy_ecs::{
    component::Component,
    error::Result,
    observer::Trigger,
    query::With,
    schedule::IntoScheduleConfigs,
    system::{Commands, Query},
};
use bytes::Bytes;
use core::net::{Ipv4Addr, SocketAddr};
use crossbeam_channel::{Receiver, Sender, TryRecvError};
use lightyear_core::time::Instant;
use lightyear_link::{Link, LinkPlugin, LinkReceiveSet, LinkSet, LinkStart, Linked};
use tracing::{error, trace};

/// Maximum transmission units; maximum size in bytes of a packet
pub(crate) const MTU: usize = 1472;
const LOCALHOST: SocketAddr = SocketAddr::new(core::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 0);

/// A component that facilitates communication over `crossbeam-channel`.
///
/// This acts as a transport layer, allowing messages to be sent and received
/// via in-memory channels, simulating a network link. It holds the sender
/// and receiver ends of the channels.
#[derive(Component, Clone)]
#[require(Link::new(None))]
#[require(LocalAddr(LOCALHOST))]
#[require(PeerAddr(LOCALHOST))]
pub struct CrossbeamIo {
    sender: Sender<Bytes>,
    receiver: Receiver<Bytes>,
}

impl CrossbeamIo {
    pub fn new(sender: Sender<Bytes>, receiver: Receiver<Bytes>) -> Self {
        Self { sender, receiver }
    }

    /// Create a pair of CrossbeamIo instances for local testing
    pub fn new_pair() -> (Self, Self) {
        let (sender1, receiver1) = crossbeam_channel::unbounded();
        let (sender2, receiver2) = crossbeam_channel::unbounded();

        (Self::new(sender1, receiver2), Self::new(sender2, receiver1))
    }
}

/// Bevy plugin to integrate the `CrossbeamIo` transport.
///
/// This plugin sets up the necessary systems for sending and receiving data
/// via `crossbeam-channel` when a `Link` component is present and active.
pub struct CrossbeamPlugin;

#[derive(QueryData)]
#[query_data(mutable)]
struct IOQuery {
    link: &'static mut Link,
    crossbeam_io: &'static CrossbeamIo,
    #[cfg(feature = "test_utils")]
    helper: Option<&'static lightyear_core::test::TestHelper>,
}

impl CrossbeamPlugin {
    fn link(trigger: Trigger<LinkStart>, mut commands: Commands) {
        commands.entity(trigger.target()).insert(Linked);
    }

    fn send(mut query: Query<IOQuery, With<Linked>>) -> Result {
        query.iter_mut().try_for_each(|mut io| {
            io.link.send.drain().try_for_each(|payload| {
                #[cfg(feature = "test_utils")]
                if io.helper.is_some_and(|h| h.block_send) {
                    return Ok(());
                }
                io.crossbeam_io.sender.try_send(payload)
            })
        })?;
        Ok(())
    }

    fn receive(mut query: Query<(&mut Link, &mut CrossbeamIo), With<Linked>>) {
        query.par_iter_mut().for_each(|(mut link, crossbeam_io)| {
            // Try to receive all available messages
            loop {
                match crossbeam_io.receiver.try_recv() {
                    Ok(data) => {
                        trace!("recv data: {data:?}");
                        link.recv.push(data, Instant::now())
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        error!("CrossbeamIO channel is disconnected");
                        break;
                    }
                }
            }
        })
    }
}

impl Plugin for CrossbeamPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<LinkPlugin>() {
            app.add_plugins(LinkPlugin);
        }
        app.add_observer(Self::link);
        app.add_systems(PreUpdate, Self::receive.in_set(LinkReceiveSet::BufferToLink));
        app.add_systems(PostUpdate, Self::send.in_set(LinkSet::Send));
    }
}
