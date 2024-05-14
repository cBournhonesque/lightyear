use bevy::ecs::system::{Command, EntityCommands};
use bevy::prelude::{Entity, World};

use crate::shared::replication::ReplicationSend;

pub struct RemoveReplicate;

fn despawn_without_replication<R: ReplicationSend>(entity: Entity, world: &mut World) {
    let mut sender = world.resource_mut::<R>();
    // remove the entity from the cache of entities that are being replicated
    // so that if it gets despawned, the despawn won't be replicated
    sender.get_mut_replicate_cache().remove(&entity);
    world.despawn(entity);
}

pub trait DespawnReplicationCommandExt<R: ReplicationSend> {
    /// Despawn the entity and makes sure that the despawn won't be replicated.
    fn despawn_without_replication(&mut self);
}
impl<R: ReplicationSend> DespawnReplicationCommandExt<R> for EntityCommands<'_> {
    fn despawn_without_replication(&mut self) {
        self.add(despawn_without_replication::<R>);
    }
}

#[cfg(test)]
mod tests {
    use bevy::utils::Duration;

    use crate::client::sync::SyncConfig;
    use crate::prelude::client::{InterpolationConfig, PredictionConfig};
    use crate::prelude::{server, LinkConditionerConfig, Replicate, SharedConfig, TickConfig};
    use crate::tests::protocol::*;
    use crate::tests::stepper::{BevyStepper, Step};

    use super::*;

    // TODO: simplify tests, we don't need a client-server connection here
    #[test]
    fn test_despawn() {
        let tick_duration = Duration::from_millis(10);
        let frame_duration = Duration::from_millis(10);
        let shared_config = SharedConfig {
            tick: TickConfig::new(tick_duration),
            ..Default::default()
        };
        let link_conditioner = LinkConditionerConfig {
            incoming_latency: Duration::from_millis(0),
            incoming_jitter: Duration::from_millis(0),
            incoming_loss: 0.0,
        };
        let sync_config = SyncConfig::default().speedup_factor(1.0);
        let prediction_config = PredictionConfig::default();
        let interpolation_config = InterpolationConfig::default();
        let mut stepper = BevyStepper::new(
            shared_config,
            sync_config,
            prediction_config,
            interpolation_config,
            link_conditioner,
            frame_duration,
        );
        stepper.init();

        let entity = stepper
            .server_app
            .world
            .spawn((Component1(1.0), Replicate::default()))
            .id();
        stepper.frame_step();
        stepper.frame_step();
        assert!(stepper
            .client_app
            .world
            .query::<&Component1>()
            .get_single(&stepper.client_app.world)
            .is_ok());

        // if we remove the Replicate component, and then despawn the entity
        // the despawn still gets replicated
        stepper
            .server_app
            .world
            .entity_mut(entity)
            .remove::<Replicate>();
        stepper.server_app.world.entity_mut(entity).despawn();
        stepper.frame_step();
        stepper.frame_step();

        assert!(stepper
            .client_app
            .world
            .query::<&Component1>()
            .get_single(&stepper.client_app.world)
            .is_err());

        // spawn a new entity
        let entity = stepper
            .server_app
            .world
            .spawn((Component1(1.0), Replicate::default()))
            .id();
        stepper.frame_step();
        stepper.frame_step();
        assert!(stepper
            .client_app
            .world
            .query::<&Component1>()
            .get_single(&stepper.client_app.world)
            .is_ok());

        // apply the command to remove replicate
        despawn_without_replication::<server::ConnectionManager>(
            entity,
            &mut stepper.server_app.world,
        );
        stepper.frame_step();
        stepper.frame_step();
        // now the despawn should not have been replicated
        assert!(stepper
            .client_app
            .world
            .query::<&Component1>()
            .get_single(&stepper.client_app.world)
            .is_ok());
    }
}
