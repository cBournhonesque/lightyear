use crate::shared::{messages::message_channel::{ChannelReader, ChannelWriter}, Message, Messages, NetEntityConverter};
use lightyear_serde::{BitReader, BitWrite, Error};

/// Struct to read/write Component/Message data into a packet.
/// (handles net entity conversion)
pub struct ProtocolIo<'c> {
    converter: &'c dyn NetEntityConverter,
}
// pub struct ProtocolIo<'c, M: Serde + EntityHolder> {
//     converter: &'c dyn NetEntityConverter,
// }

impl<'c> ProtocolIo<'c> {
    pub fn new(converter: &'c dyn NetEntityConverter) -> Self {
        Self { converter }
    }
}

impl<'c> ChannelWriter<Box<dyn Message>> for ProtocolIo<'c> {
    fn write(&self, writer: &mut dyn BitWrite, data: &Box<dyn Message>) {
        Messages::write(writer, self.converter, data);
    }
}

impl<'c> ChannelReader<Box<dyn Message>> for ProtocolIo<'c> {
    fn read(&self, reader: &mut BitReader) -> Result<Box<dyn Message>, Error> {
        Messages::read(reader, self.converter)
    }
}
