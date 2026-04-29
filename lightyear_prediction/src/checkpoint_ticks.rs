use bevy_replicon::prelude::RepliconTick;
use lightyear_core::tick::Tick;
use lightyear_replication::checkpoint::ReplicationCheckpointMap;
use lightyear_replication::prelude::{ConfirmHistory, ServerMutateTicks};

pub(crate) fn resolve_message_tick(
    checkpoints: &ReplicationCheckpointMap,
    tick: RepliconTick,
) -> Option<Tick> {
    checkpoints.get(tick)
}

pub(crate) fn resolve_confirm_history_tick(
    checkpoints: &ReplicationCheckpointMap,
    history: &ConfirmHistory,
) -> Option<Tick> {
    resolve_message_tick(checkpoints, history.last_tick())
}

pub(crate) fn resolve_server_mutate_tick(
    checkpoints: &ReplicationCheckpointMap,
    ticks: &ServerMutateTicks,
) -> Option<Tick> {
    resolve_message_tick(checkpoints, ticks.last_tick())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_authoritative_tick_for_large_replicon_gap() {
        let mut checkpoints = ReplicationCheckpointMap::default();
        checkpoints.record(RepliconTick::new(200), Tick(20));

        assert_eq!(
            resolve_message_tick(&checkpoints, RepliconTick::new(200)),
            Some(Tick(20))
        );
    }

    #[test]
    fn collapses_multiple_replicon_ticks_for_same_authoritative_tick() {
        let mut checkpoints = ReplicationCheckpointMap::default();
        checkpoints.record(RepliconTick::new(100), Tick(20));
        checkpoints.record(RepliconTick::new(101), Tick(20));

        assert_eq!(
            resolve_message_tick(&checkpoints, RepliconTick::new(100)),
            Some(Tick(20))
        );
        assert_eq!(
            resolve_message_tick(&checkpoints, RepliconTick::new(101)),
            Some(Tick(20))
        );
    }

    #[test]
    fn resync_does_not_reinterpret_confirmed_checkpoint_tick() {
        let mut checkpoints = ReplicationCheckpointMap::default();
        checkpoints.record(RepliconTick::new(50), Tick(20));

        let mut history = ConfirmHistory::new(RepliconTick::new(50));
        history.confirm(RepliconTick::new(49));

        assert_eq!(
            resolve_confirm_history_tick(&checkpoints, &history),
            Some(Tick(20))
        );
    }
}
