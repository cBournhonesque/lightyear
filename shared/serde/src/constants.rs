const MIN_FRAGMENTATION_THRESHOLD_SIZE_BYTES: usize = 576;
const IP_HEADER_SIZE_BYTES: usize = 60;
const UDP_HEADER_SIZE_BYTES: usize = 8;
const DTLS_HEADER_SIZE_BYTES: usize = 50;
const SCTP_HEADER_SIZE_BYTES: usize = 28;
/// The maximum of bytes that can be used for the payload of a given packet.
/// (See #38 of <http://ithare.com/64-network-dos-and-donts-for-game-engines-part-v-udp/>)
pub const MTU_SIZE_BYTES: usize = MIN_FRAGMENTATION_THRESHOLD_SIZE_BYTES
    - IP_HEADER_SIZE_BYTES
    - UDP_HEADER_SIZE_BYTES
    - DTLS_HEADER_SIZE_BYTES
    - SCTP_HEADER_SIZE_BYTES;
pub const MTU_SIZE_BITS: u32 = (MTU_SIZE_BYTES * 8) as u32;
