use crate::transport::Transport;

pub(crate) struct ConnectedTransport {
    // transport to transmit raw packets to an address
    transport: Box<dyn Transport>,
}
