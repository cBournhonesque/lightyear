use bevy::ecs::system::{Command, EntityCommands};
use bevy::prelude::{Entity, World};

use crate::_reexport::ReplicationSend;
use crate::prelude::Protocol;
use crate::shared::replication::components::Replicate;

pub struct RemoveReplicate;

fn remove_replicate<P: Protocol, R: ReplicationSend<P>>(entity: Entity, world: &mut World) {
    let mut sender = world.resource_mut::<R>();
    // remove the entity from the cache of entities that are being replicated
    // so that if it gets despawned, the despawn won't be replicated
    sender.get_mut_replicate_component_cache().remove(&entity);
    // remove the replicate component
    if let Some(mut entity) = world.get_entity_mut(entity) {
        entity.remove::<Replicate<P>>();
    }
}

pub trait RemoveReplicateCommandsExt<P: Protocol, R: ReplicationSend<P>> {
    /// Remove the replicate component from the entity.
    /// This also makes sure that if you despawn the entity right after, the despawn won't be replicated.
    ///
    /// This can be useful when you want to despawn an entity on the server, but you don't want the despawn to be replicated
    /// immediately to clients (for example because clients are playing a despawn animation)/
    fn remove_replicate(&mut self);
}
impl<P: Protocol, R: ReplicationSend<P>> RemoveReplicateCommandsExt<P, R> for EntityCommands<'_> {
    fn remove_replicate(&mut self) {
        self.add(remove_replicate::<P, R>);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::sync::SyncConfig;
    use crate::prelude::client::{InterpolationConfig, PredictionConfig};
    use crate::prelude::{LinkConditionerConfig, SharedConfig, TickConfig};
    use crate::server::connection::ConnectionManager;
    use crate::server::replication::ServerReplicationPlugin;
    use crate::tests::protocol::Replicate;
    use crate::tests::protocol::*;
    use crate::tests::stepper::{BevyStepper, Step};
    use bevy::prelude::*;
    use bevy::utils::Duration;

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
        let prediction_config = PredictionConfig::default().disable(false);
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
        remove_replicate::<MyProtocol, ServerConnectionManager>(
            entity,
            &mut stepper.server_app.world,
        );
        stepper.server_app.world.entity_mut(entity).despawn();
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
