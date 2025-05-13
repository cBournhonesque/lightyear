//! # Lightyear Crossbeam
//!
//! This crate provides a transport layer for Lightyear that uses `crossbeam-channel`.
//! It's primarily intended for local testing or scenarios where in-process message passing
//! is desired, simulating a network connection without actual network I/O.
//!
//! It defines `CrossbeamIo` for channel-based communication and `CrossbeamPlugin`
//! to integrate this transport into a Bevy application.
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

use bevy::platform::time::Instant;
use bevy::prelude::*;
use bytes::Bytes;
use core::net::{Ipv4Addr, SocketAddr};
use crossbeam_channel::{Receiver, Sender, TryRecvError};
use lightyear_link::{Link, LinkPlugin, LinkSet, LinkStart, Linked, Unlink, Unlinked};
use tracing::error;

/// Maximum transmission units; maximum size in bytes of a packet
pub(crate) const MTU: usize = 1472;
const LOCALHOST: SocketAddr = SocketAddr::new(core::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 0);

/// A component that facilitates communication over `crossbeam-channel`.
///
/// This acts as a transport layer, allowing messages to be sent and received
/// via in-memory channels, simulating a network link. It holds the sender
/// and receiver ends of the channels.
#[derive(Component)]
#[require(Link::new(LOCALHOST, None))]
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

impl CrossbeamPlugin {
    fn link(trigger: Trigger<LinkStart>, mut commands: Commands) {
        commands.entity(trigger.target()).insert(Linked);
    }

    fn send(mut query: Query<(&mut Link, &mut CrossbeamIo), With<Linked>>) -> Result {
        query
            .iter_mut()
            .try_for_each(|(mut link, mut crossbeam_io)| {
                link.send.drain().try_for_each(|payload| {
                    trace!("send data: {payload:?}");
                    crossbeam_io.sender.try_send(payload)
                })
            })?;
        Ok(())
    }

    fn receive(mut query: Query<(&mut Link, &mut CrossbeamIo), With<Linked>>) {
        query
            .par_iter_mut()
            .for_each(|(mut link, mut crossbeam_io)| {
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
        app.add_systems(PreUpdate, Self::receive.in_set(LinkSet::Receive));
        app.add_systems(PostUpdate, Self::send.in_set(LinkSet::Send));
    }
}
