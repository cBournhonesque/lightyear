//! Wrapper around a transport, that can perform additional transformations such as
//! bandwidth monitoring or compression
pub(crate) mod config;
pub(crate) mod transport;

use crate::transport::error::{Error, Result};
use crate::transport::io::{BaseIo, IoState};
use bevy::prelude::{Deref, DerefMut};
use crossbeam_channel::Sender;

#[cfg(feature = "std")]
use std::{io};
#[cfg(not(feature = "std"))]
use {
    no_std_io2::io,
};
use core::net::SocketAddr;

pub struct IoContext {
    pub(crate) event_sender: Option<ServerNetworkEventSender>,
    pub(crate) event_receiver: Option<ServerIoEventReceiver>,
}

/// Server IO
pub type Io = BaseIo<IoContext>;

impl Io {
    pub fn close(&mut self) -> Result<()> {
        self.state = IoState::Disconnected;
        if let Some(event_sender) = self.context.event_sender.as_mut() {
            event_sender
                .try_send(ServerIoEvent::ServerDisconnected(
                    io::Error::other("server requested disconnection").into(),
                ))
                .map_err(Error::from)?;
        }
        Ok(())
    }
}

#[derive(Deref, DerefMut, Clone)]
pub(crate) struct ServerIoEventReceiver(pub(crate) async_channel::Receiver<ServerIoEvent>);

/// Events that will be sent from the io thread to the main thread
pub(crate) enum ServerIoEvent {
    ServerConnected,
    ServerDisconnected(Error),
    ClientDisconnected(SocketAddr),
}

/// Events that will be sent from the main thread to the io thread
#[derive(Deref, DerefMut, Clone)]
pub(crate) struct ServerNetworkEventSender(pub(crate) async_channel::Sender<ServerIoEvent>);
