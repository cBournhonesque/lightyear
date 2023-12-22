/*!  A connection is a wrapper that lets us send message and apply replication
*/
// only public for proc macro
pub mod events;

pub(crate) mod message;
mod send;
