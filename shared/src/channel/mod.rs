pub mod channel;

pub(crate) mod receivers;
pub(crate) mod senders;

use crate::packet::wrapping_id::wrapping_id;

// intend to act as a wrapper around MessageId or ComponentId ?
// wrapping_id!(MessageIndex);
