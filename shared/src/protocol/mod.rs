pub(crate) mod channel;
pub(crate) mod message;

pub trait Protocol {
    type Message: message::MessageProtocol;
    type Channel: channel::ChannelProtocol;

    fn get_message_protocol() -> Self::Message;
    fn get_channel_protocol() -> Self::Channel;
}
