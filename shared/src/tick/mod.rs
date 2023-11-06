pub(crate) mod manager;
pub(crate) mod message;
pub(crate) mod ping_store;
pub(crate) mod time;

use crate::utils::wrapping_id;

/// Internal id that tracks the Tick value for the server and the client
wrapping_id!(Tick);
