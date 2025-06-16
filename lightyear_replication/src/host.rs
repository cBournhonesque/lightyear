use crate::buffer::Replicate;
use crate::control::{Controlled, ControlledBy};
#[cfg(feature = "interpolation")]
use crate::prelude::InterpolationTarget;
#[cfg(feature = "prediction")]
use crate::prelude::PredictionTarget;
use bevy::ecs::query::QueryData;
use bevy::prelude::*;
use lightyear_connection::host::HostClient;
use lightyear_core::interpolation::Interpolated;
#[cfg(feature = "prediction")]
use lightyear_core::prediction::Predicted;

// impl ControlledBy {
//     /// In Host-Server mode, any entity that is marked as ControlledBy the host
//     /// should also have Controlled assigned to them
//     /// (because most client queries have Controlled as filter)
//     pub(crate) fn on_add_host_server(
//         trigger: Trigger<OnAdd, ControlledBy>,
//         query: Query<&ControlledBy>,
//         owner: Query<(), With<HostClient>>,
//         mut commands: Commands,
//     ) {
//         if owner.get(query.get(trigger.target()).unwrap().owner).is_ok() {
//             commands.entity(trigger.target()).insert(Controlled);
//         }
//     }
// }

pub struct HostServerPlugin;


#[derive(QueryData)]
struct HostServerQueryData {
    entity: Entity,
    replicate: Ref<'static, Replicate>,
    #[cfg(feature = "prediction")]
    prediction: Option<&'static PredictionTarget>,
    #[cfg(feature = "interpolation")]
    interpolation: Option<&'static InterpolationTarget>,
    controlled: Option<Ref<'static, ControlledBy>>,
}

impl HostServerPlugin {

    // TODO: try to do this cheaper with observers?
    /// In HostServer mode, we will add the Predicted/Interpolated components to the entities
    /// that the host-server wants to replicate, so that client code can still query for them
    fn add_prediction_interpolation_components(
        mut commands: Commands,
        local_client: Single<Entity, With<HostClient>>,
        query: Query<HostServerQueryData>,
    ) {
        let local_entity = local_client.into_inner();
        query.iter().for_each(|d| {
            // also insert [`Controlled`] on the entity if it's controlled by the local client
            if let Some(controlled_by) = d.controlled {
                if controlled_by.is_changed() && controlled_by.owner == local_entity {
                    commands
                        .entity(local_entity)
                        .insert(Controlled);
                }
            }
            if d.replicate.is_changed() && d.replicate.senders.contains(&local_entity) {
                #[cfg(feature = "prediction")]
                if d.prediction.is_some_and(|p| p.senders.contains(&local_entity)) {
                    commands.entity(d.entity).insert(Predicted {
                        confirmed_entity: Some(d.entity)
                    });
                }
                
                #[cfg(feature = "interpolation")]
                if d.interpolation.is_some_and(|p| p.senders.contains(&local_entity)) {
                    commands.entity(d.entity).insert(Interpolated {
                        confirmed_entity: d.entity
                    });
                }
            }
        });
    }
}

impl Plugin for HostServerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PostUpdate, Self::add_prediction_interpolation_components);
    }
}