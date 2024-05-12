use std::sync::OnceLock;

use bevy::utils::synccell::SyncCell;
use steamworks::{ClientManager, SingleClient};
use tracing::info;

pub struct SteamworksClient {
    app_id: u32,
    client: steamworks::Client<ClientManager>,
    single: SyncCell<SingleClient>,
}

impl std::fmt::Debug for SteamworksClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SteamworksClient")
            .field("app_id", &self.app_id)
            .finish()
    }
}

impl SteamworksClient {
    pub fn new(app_id: u32) -> Self {
        let (client, single) = steamworks::Client::<ClientManager>::init_app(app_id).unwrap();

        Self {
            app_id,
            client,
            single: SyncCell::new(single),
        }
    }

    pub fn get_client(&self) -> steamworks::Client<ClientManager> {
        self.client.clone()
    }

    pub fn get_single(&mut self) -> &mut SingleClient<ClientManager> {
        self.single.get()
    }
}
