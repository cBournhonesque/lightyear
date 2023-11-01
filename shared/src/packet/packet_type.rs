use bitcode::{Decode, Encode};
use serde::{Deserialize, Serialize};

#[derive(Copy, Debug, Clone, Eq, PartialEq, Serialize, Deserialize, Encode, Decode)]
pub enum PacketType {
    // A packet containing actual data
    #[bitcode_hint(frequency = 100)]
    Data,
    // A packet sent to maintain the connection by preventing a timeout
    #[bitcode_hint(frequency = 50)]
    KeepAlive,
    // A Ping message, used to calculate RTT. Must be responded to with a Pong
    // message
    Ping,
    // A Pong message, used to calculate RTT. Must be the response to all Ping
    // messages
    Pong,
    // A packet containing actual data, but which is fragmented into multiple parts
    #[bitcode_hint(frequency = 5)]
    DataFragment,
}
