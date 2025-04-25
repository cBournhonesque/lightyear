use crate::prelude::LinkConditionerConfig;
#[cfg(not(feature = "std"))]
use alloc::{vec, vec::Vec};
use steamworks::networking_types::{NetworkingConfigEntry, NetworkingConfigValue};

pub(crate) mod client;
pub(crate) mod server;
pub(crate) mod steamworks_client;

pub(crate) fn get_networking_options(
    conditioner: &Option<LinkConditionerConfig>,
) -> Vec<NetworkingConfigEntry> {
    let mut options = vec![NetworkingConfigEntry::new_int32(
        NetworkingConfigValue::NagleTime,
        0,
    )];
    if let Some(conditioner) = conditioner {
        // TODO: float options are not useable, see https://github.com/Noxime/steamworks-rs/pull/168
        // options.push(NetworkingConfigEntry::new_float(
        //     NetworkingConfigValue::FakePacketLossRecv,
        //     conditioner.incoming_loss * 100.0,
        // ));
        options.push(NetworkingConfigEntry::new_int32(
            NetworkingConfigValue::FakePacketLagRecv,
            conditioner.incoming_latency.as_millis() as i32,
        ));
        options.push(NetworkingConfigEntry::new_int32(
            NetworkingConfigValue::FakePacketReorderTime,
            conditioner.incoming_jitter.as_millis() as i32,
        ));
        // TODO: float options are not useable, see https://github.com/Noxime/steamworks-rs/pull/168
        // options.push(NetworkingConfigEntry::new_float(
        //     NetworkingConfigValue::FakePacketReorderRecv,
        //     100.0,
        // ));
    }
    options
}
