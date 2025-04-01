/*! # Lightyear Crossbeam

Low-level IO primitives for the lightyear networking library.
This crate provides abstractions for sending and receiving raw bytes over the network.
*/
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

use bevy::prelude::*;
use bytes::Bytes;
use crossbeam_channel::{Receiver, Sender, TryRecvError};
use lightyear_link::{Link, LinkSet};
use tracing::error;

/// Maximum transmission units; maximum size in bytes of a packet
pub(crate) const MTU: usize = 1472;


#[derive(Component)]
#[require(Link)]
pub struct CrossbeamIo {
    sender: Sender<Bytes>,
    receiver: Receiver<Bytes>,
}

impl CrossbeamIo {
    pub fn new(sender: Sender<Bytes>, receiver: Receiver<Bytes>) -> Self {
        Self {
            sender,
            receiver,
        }
    }
    
    /// Create a pair of CrossbeamIo instances for local testing
    pub fn new_pair() -> (Self, Self) {
        let (sender1, receiver1) = crossbeam_channel::unbounded();
        let (sender2, receiver2) = crossbeam_channel::unbounded();
        
        (
            Self::new(sender1, receiver2),
            Self::new(sender2, receiver1),
        )
    }
}

pub struct CrossbeamPlugin;

impl CrossbeamPlugin {
    fn send(
        mut query: Query<(&mut Link, &mut CrossbeamIo)>
    ) -> Result {
        // TODO: parallelize by using crossbeam channels inside Link!
        query.iter_mut().try_for_each(|(mut link, mut crossbeam_io)| {
            link.send.drain(..).try_for_each(|payload| {
                crossbeam_io.sender
                    .try_send(payload)
            })
        })?;
        Ok(())
    }

    fn receive(
        mut query: Query<(&mut Link, &mut CrossbeamIo)>
    ) {
        // TODO: parallelize
        query.iter_mut().for_each(|(mut link, mut crossbeam_io)| {
            // Try to receive all available messages
            loop {
                match  crossbeam_io.receiver.try_recv() {
                    Ok(data) => {link.recv.push(data)}
                    Err(TryRecvError::Empty) => {break},
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
        app.add_systems(PreUpdate, Self::receive.in_set(LinkSet::Receive));
        app.add_systems(PreUpdate, Self::send.in_set(LinkSet::Send));
    }
}
