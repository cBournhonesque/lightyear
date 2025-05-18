use bevy::prelude::*;
use lightyear_link::Linked;
use lightyear_link::prelude::LinkOf;
use lightyear_messages::MessageManager;
use lightyear_replication::components::{PrePredicted, Replicated};
use lightyear_replication::prelude::ReplicationSender;

pub struct ServerPlugin;

impl Plugin for ServerPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(crate::shared::SharedPlugin);
        app.add_observer(Self::handle_pre_predicted_server);
    }
}

impl ServerPlugin {
    /// When we receive an entity that a clients wants PrePredicted,
    /// we immediately transfer authority back to the server. The server will replicate the PrePredicted
    /// component back to the client. Upon receipt, the client will replace PrePredicted with Predicted.
    ///
    /// The entity mapping is done on the client.
    pub(crate) fn handle_pre_predicted_server(
        trigger: Trigger<OnAdd, PrePredicted>,
        mut commands: Commands,
        mut link: Query<&mut MessageManager, (With<ReplicationSender>, With<LinkOf>, With<Linked>)>,
        q: Query<(Entity, &PrePredicted, &Replicated)>,
    ) {
        if let Ok((local_entity, pre_predicted, replicated)) = q.get(trigger.target()) {
            let sending_client = replicated.from;
            // if the client who created the PrePredicted entity is the local client, no need to do anything!
            // (the client Observer already adds Predicted on the entity)
            if sending_client.is_local() {
                return;
            }
            if let Ok(mut message_manager) = link.get_mut(replicated.receiver) {
                // we remove Replicated but we keep InitialReplicated
                commands.entity(local_entity).remove::<Replicated>();
                let confirmed_entity = pre_predicted.confirmed_entity.unwrap();
                // update the mapping so that when we send updates, the server entity gets mapped
                // to the client's confirmed entity
                message_manager
                    .entity_mapper
                    .insert(confirmed_entity, local_entity);
                trace!(
                    ?confirmed_entity,
                    ?local_entity,
                    "Received PrePredicted entity from client: {:?}. Updating entity map on server",
                    replicated.from
                );
            }
        }
    }
}
