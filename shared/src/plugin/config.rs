use crate::TickConfig;

#[derive(Clone)]
pub struct SharedConfig {
    pub enable_replication: bool,
    pub tick: TickConfig,
}
