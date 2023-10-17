use lightyear_shared::IoConfig;

pub struct ClientConfig {
    pub netcode: lightyear_shared::netcode::ClientConfig<()>,
    pub io: IoConfig,
}
