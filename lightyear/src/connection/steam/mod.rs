use std::sync::{Arc, RwLock};

pub(crate) mod client;
pub(crate) mod server;

pub(crate) struct SingleClientThreadSafe(Arc<RwLock<steamworks::SingleClient>>);

unsafe impl Sync for SingleClientThreadSafe {}
unsafe impl Send for SingleClientThreadSafe {}
