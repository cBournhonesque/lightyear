//! Wrapper around a transport, that can perform additional transformations such as
//! bandwidth monitoring or compression
pub(crate) mod config;
pub(crate) mod transport;

#[cfg(not(feature = "std"))]
use no_std_io2::io;
#[cfg(feature = "std")]
use std::io;

use crate::transport::error::{Error, Result};
use crate::transport::io::{BaseIo, IoState};
use async_channel::{Receiver, Sender};
use bevy::prelude::{Deref, DerefMut};

pub struct IoContext {
    pub(crate) event_sender: Option<ClientNetworkEventSender>,
    pub(crate) event_receiver: Option<ClientIoEventReceiver>,
}

/// Client IO
pub type Io = BaseIo<IoContext>;

impl Io {
    pub fn close(&mut self) -> Result<()> {
        self.state = IoState::Disconnected;
        if let Some(event_sender) = self.context.event_sender.as_mut() {
            event_sender
                .try_send(ClientIoEvent::Disconnected(
                    io::Error::other("client requested disconnection").into(),
                ))
                .map_err(Error::from)?;
        }
        Ok(())
    }
}

/// Events that will be sent from the io thread to the main thread
/// (so that we can update the netcode state when the io changes)
pub(crate) enum ClientIoEvent {
    Connected,
    Disconnected(Error),
}

#[derive(Deref, DerefMut)]
pub(crate) struct ClientIoEventReceiver(pub(crate) Receiver<ClientIoEvent>);

#[derive(Deref, DerefMut)]
pub(crate) struct ClientNetworkEventSender(pub(crate) Sender<ClientIoEvent>);
