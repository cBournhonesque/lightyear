use bevy::prelude::*;
use bevy::time::common_conditions::on_timer;
use bevy::utils::Duration;

use crate::_reexport::{ComponentProtocol, ReplicationSend};
use crate::prelude::{MainSet, Protocol, ReplicationSet};
use crate::shared::replication::hierarchy::HierarchySyncPlugin;
use crate::shared::replication::systems::{add_replication_send_systems, cleanup};

pub(crate) struct ReplicationPlugin<P: Protocol, R: ReplicationSend<P>> {
    tick_duration: Duration,
    _marker: std::marker::PhantomData<(P, R)>,
}

impl<P: Protocol, R: ReplicationSend<P>> ReplicationPlugin<P, R> {
    pub(crate) fn new(tick_duration: Duration) -> Self {
        Self {
            tick_duration,
            _marker: std::marker::PhantomData,
        }
    }
}

impl<P: Protocol, R: ReplicationSend<P>> Plugin for ReplicationPlugin<P, R> {
    fn build(&self, app: &mut App) {
        // TODO: have a better constant for clean_interval?
        let clean_interval = self.tick_duration * (i16::MAX as u32 / 3);

        // SYSTEM SETS //
        // NOTE: it's ok to run the replication systems less frequently than every frame
        //  because bevy's change detection detects changes since the last time the system ran (not since the last frame)
        app.configure_sets(
            PostUpdate,
            (
                (
                    ReplicationSet::<R::SetMarker>::SendEntityUpdates,
                    ReplicationSet::<R::SetMarker>::SendComponentUpdates,
                    ReplicationSet::<R::SetMarker>::SendDespawnsAndRemovals,
                )
                    .in_set(ReplicationSet::<R::SetMarker>::All),
                (
                    ReplicationSet::<R::SetMarker>::SendEntityUpdates,
                    ReplicationSet::<R::SetMarker>::SendComponentUpdates,
                    // NOTE: SendDespawnsAndRemovals is not in MainSet::Send because we need to run them every frame
                    MainSet::<R::SetMarker>::SendPackets,
                )
                    .in_set(MainSet::<R::SetMarker>::Send),
                (
                    ReplicationSet::<R::SetMarker>::All,
                    MainSet::<R::SetMarker>::SendPackets,
                )
                    .chain(),
            ),
        );

        // SYSTEMS
        add_replication_send_systems::<P, R>(app);
        P::Components::add_per_component_replication_send_systems::<R>(app);
        app.add_systems(Last, cleanup::<P, R>.run_if(on_timer(clean_interval)));

        // PLUGINS
        app.add_plugins(HierarchySyncPlugin::<P, R>::default());
    }
}
