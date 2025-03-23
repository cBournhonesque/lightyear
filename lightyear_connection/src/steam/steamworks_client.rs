use bevy::utils::synccell::SyncCell;
use steamworks::{ClientManager, SIResult, SingleClient};

/// This wraps the Steamworks client. It must only be created once per
/// application run. For convenience, Lightyear can automatically create the
/// client for you, but for more control, you can create it yourself and pass it in to Lightyear.
pub struct SteamworksClient {
    app_id: u32,
    client: steamworks::Client<ClientManager>,
    single: SyncCell<SingleClient>, // https://github.com/Noxime/steamworks-rs/issues/159
}

impl core::fmt::Debug for SteamworksClient {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SteamworksClient")
            .field("app_id", &self.app_id)
            .finish()
    }
}

impl SteamworksClient {
    /// Creates and initializes the Steamworks client with the specified AppId.
    /// This must only be called once per application run.
    pub fn new_with_app_id(app_id: u32) -> SIResult<Self> {
        let (client, single) = steamworks::Client::<ClientManager>::init_app(app_id)?;

        Ok(Self {
            app_id,
            client,
            single: SyncCell::new(single),
        })
    }

    /// Creates and initializes the Steamworks client. If the game isn’t being run through steam
    /// this can be provided by placing a steam_appid.txt with the ID inside in the current
    /// working directory.
    /// This must only be called once per application run.
    pub fn new() -> SIResult<Self> {
        let (client, single) = steamworks::Client::<ClientManager>::init()?;

        Ok(Self {
            app_id: client.utils().app_id().0,
            client,
            single: SyncCell::new(single),
        })
    }

    /// Gets the thread-safe Steamworks client. Most Steamworks API calls live
    /// under this client.
    pub fn get_client(&self) -> steamworks::Client<ClientManager> {
        self.client.clone()
    }

    /// Gets the non-thread-safe Steamworks client. This is only used to run
    /// Steamworks callbacks.
    pub fn get_single(&mut self) -> &mut SingleClient<ClientManager> {
        self.single.get()
    }
}
