// An enum representing the different types of packets that can be
// sent/received

use lightyear_serde::{BitReader, BitWrite, SerdeErr, UnsignedInteger};

#[derive(Copy, Debug, Clone, Eq, PartialEq)]
pub enum PacketType {
    // A packet containing Message/Entity/Component data
    Data,
    // A packet sent to maintain the connection by preventing a timeout
    Heartbeat,
    // An initial handshake message sent by the Client to the Server
    ClientChallengeRequest,
    // The Server's response to the Client's initial handshake message
    ServerChallengeResponse,
    // The final handshake message sent by the Client
    ClientConnectRequest,
    // The final handshake message sent by the Server, indicating that the
    // connection has been established
    ServerConnectResponse,
    // Indicates that the authentication payload was rejected, handshake must restart
    ServerRejectResponse,
    // A Ping message, used to calculate RTT. Must be responded to with a Pong
    // message
    Ping,
    // A Pong message, used to calculate RTT. Must be the response to all Ping
    // messages
    Pong,
    // Used to request a graceful Client disconnect from the Server
    Disconnect,
}

// Most packets should be Data, so lets compress this a bit more.
// Could do this with another enum, but code would get messy.
impl lightyear_serde::Serde for PacketType {
    fn ser(&self, writer: &mut dyn BitWrite) {
        let is_data = *self == PacketType::Data;
        is_data.ser(writer);

        if is_data {
            return;
        }

        let index = match self {
            PacketType::Data => panic!("shouldn't happen, caught above"),
            PacketType::Heartbeat => 0,
            PacketType::ClientChallengeRequest => 1,
            PacketType::ServerChallengeResponse => 2,
            PacketType::ClientConnectRequest => 3,
            PacketType::ServerConnectResponse => 4,
            PacketType::ServerRejectResponse => 5,
            PacketType::Ping => 6,
            PacketType::Pong => 7,
            PacketType::Disconnect => 8,
        };

        UnsignedInteger::<4>::new(index).ser(writer);
    }

    fn de(reader: &mut BitReader) -> Result<Self, SerdeErr> {
        let is_data = bool::de(reader)?;
        if is_data {
            return Ok(PacketType::Data);
        }

        match UnsignedInteger::<4>::de(reader)?.get() {
            0 => Ok(PacketType::Heartbeat),
            1 => Ok(PacketType::ClientChallengeRequest),
            2 => Ok(PacketType::ServerChallengeResponse),
            3 => Ok(PacketType::ClientConnectRequest),
            4 => Ok(PacketType::ServerConnectResponse),
            5 => Ok(PacketType::ServerRejectResponse),
            6 => Ok(PacketType::Ping),
            7 => Ok(PacketType::Pong),
            8 => Ok(PacketType::Disconnect),
            _ => panic!("shouldn't happen, caught above"),
        }
    }
}
