use serde::{Deserialize, Serialize};

use bitcode::{Decode, Encode};

#[derive(Copy, Debug, Clone, Eq, PartialEq, Serialize, Deserialize, Encode, Decode)]
pub enum PacketType {
    // A packet containing actual data
    #[bitcode_hint(frequency = 100)]
    Data,
    // // A packet sent to maintain the connection by preventing a timeout
    // #[bitcode_hint(frequency = 50)]
    // KeepAlive,
    // // A Ping message, used to calculate RTT. Must be responded to with a Pong
    // // message
    // #[bitcode_hint(frequency = 1)]
    // Ping,
    // // A Pong message, used to calculate RTT. Must be the response to all Ping
    // // messages
    // #[bitcode_hint(frequency = 1)]
    // Pong,
    // A packet containing actual data, but which is fragmented into multiple parts
    #[bitcode_hint(frequency = 5)]
    DataFragment,
}
