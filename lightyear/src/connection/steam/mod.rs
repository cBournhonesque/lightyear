use crate::prelude::LinkConditionerConfig;
use steamworks::networking_types::{NetworkingConfigEntry, NetworkingConfigValue};

pub(crate) mod client;
pub(crate) mod server;

// NOTE: it looks like there's SingleClient can actually be called on multiple threads
// - https://partner.steamgames.com/doc/api/steam_api#SteamAPI_RunCallbacks
pub(crate) struct SingleClientThreadSafe(steamworks::SingleClient);

unsafe impl Sync for SingleClientThreadSafe {}
unsafe impl Send for SingleClientThreadSafe {}

pub(crate) fn get_networking_options(
    conditioner: &Option<LinkConditionerConfig>,
) -> Vec<NetworkingConfigEntry> {
    let mut options = vec![NetworkingConfigEntry::new_int32(
        NetworkingConfigValue::NagleTime,
        0,
    )];
    if let Some(ref conditioner) = conditioner {
        options.push(NetworkingConfigEntry::new_float(
            NetworkingConfigValue::FakePacketLossRecv,
            conditioner.incoming_loss * 100.0,
        ));
        options.push(NetworkingConfigEntry::new_int32(
            NetworkingConfigValue::FakePacketLagRecv,
            conditioner.incoming_latency.as_millis() as i32,
        ));
        options.push(NetworkingConfigEntry::new_int32(
            NetworkingConfigValue::FakePacketReorderTime,
            conditioner.incoming_jitter.as_millis() as i32,
        ));
        options.push(NetworkingConfigEntry::new_float(
            NetworkingConfigValue::FakePacketReorderRecv,
            100.0,
        ));
    }
    options
}
