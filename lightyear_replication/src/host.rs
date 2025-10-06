use crate::components::{ComponentReplicationOverrides, InitialReplicated, Replicated};
use crate::control::{Controlled, ControlledBy};
use crate::hierarchy::ReplicateLike;
#[cfg(feature = "interpolation")]
use crate::prelude::InterpolationTarget;
#[cfg(feature = "prediction")]
use crate::prelude::PredictionTarget;
use crate::prelude::Replicate;
use bevy_app::{App, Plugin, PostUpdate};
use bevy_ecs::prelude::*;
use bevy_ecs::query::QueryData;
use lightyear_connection::host::HostClient;
#[cfg(feature = "interpolation")]
use lightyear_core::interpolation::Interpolated;
#[cfg(feature = "prediction")]
use lightyear_core::prediction::Predicted;
use lightyear_core::prelude::{LocalTimeline, NetworkTimeline};
#[allow(unused_imports)]
use tracing::{debug, info};
// impl ControlledBy {
//     /// In Host-Server mode, any entity that is marked as ControlledBy the host
//     /// should also have Controlled assigned to them
//     /// (because most client queries have Controlled as filter)
//     pub(crate) fn on_add_host_server(
//         trigger: On<Add, ControlledBy>,
//         query: Query<&ControlledBy>,
//         owner: Query<(), With<HostClient>>,
//         mut commands: Commands,
//     ) {
//         if owner.get(query.get(trigger.entity).unwrap().owner).is_ok() {
//             commands.entity(trigger.entity).insert(Controlled);
//         }
//     }
// }

pub struct HostServerPlugin;

#[derive(QueryData)]
struct HostServerQueryData {
    entity: Entity,
    replicate: Option<Ref<'static, Replicate>>,
    #[cfg(feature = "prediction")]
    prediction: Option<Ref<'static, PredictionTarget>>,
    #[cfg(feature = "interpolation")]
    interpolation: Option<Ref<'static, InterpolationTarget>>,
    controlled: Option<Ref<'static, ControlledBy>>,
    replicate_like: Option<Ref<'static, ReplicateLike>>,
}

#[derive(Component)]
struct SpawnedOnHostServer;

impl HostServerPlugin {
    // TODO: try to do this cheaper with observers?
    /// In HostServer mode, we will add the Predicted/Interpolated components to the entities
    /// that the host-server wants to replicate, so that client code can still query for them
    fn add_prediction_interpolation_components(
        mut commands: Commands,
        local_client: Single<(Entity, &LocalTimeline, Ref<HostClient>)>,
        // this is only for entities that are spawned on the host-server
        // we can't rely only on Without<InitialReplicated> because we add a fake
        // InitialReplicated in this system, which might prevent fake Predicted/Interpolated
        // to be added in the future
        query: Query<
            HostServerQueryData,
            Or<(Without<InitialReplicated>, With<SpawnedOnHostServer>)>,
        >,
    ) {
        let (local_entity, timeline, host_client) = local_client.into_inner();
        let tick = timeline.tick();

        let add_fake_components =
            |commands: &mut Commands, d: &HostServerQueryDataItem, entity: Entity| {
                // TODO: r.is_changed() will trigger everytime a new client connects (because the list of senders is modified),
                //  even though the replicate's target doesn't change. To avoid this, we will use `is_added()` for now.
                //  Maybe a long term solution would be to split Replicate into ReplicationTarget (just the Mode) and
                //  ReplicateMetadata (list of senders, authority, etc). Same for Prediction/Interpolation
                if d.replicate.as_ref().is_some_and(|r| {
                    (r.is_added() || host_client.is_added()) && r.senders.contains(&local_entity)
                }) {
                    debug!(
                        "insert fake Replicated on {:?}. Replicate: {:?}",
                        entity, d.replicate
                    );
                    commands.entity(entity).insert((
                        Replicated {
                            receiver: local_entity,
                        },
                        InitialReplicated {
                            receiver: local_entity,
                        },
                        SpawnedOnHostServer,
                    ));
                }
                // also insert [`Controlled`] on the entity if it's controlled by the local client
                if d.controlled.as_ref().is_some_and(|c| {
                    (c.is_changed() || host_client.is_added()) && c.owner == local_entity
                }) {
                    commands
                        .entity(entity)
                        // NOTE: do not replicate this Controlled to other clients, or they will
                        // think they control this entity
                        .insert((
                            Controlled,
                            ComponentReplicationOverrides::<Controlled>::default().disable_all(),
                        ));
                }
                #[cfg(feature = "prediction")]
                if d.prediction.as_ref().is_some_and(|p| {
                    (p.is_added() || host_client.is_added()) && p.senders.contains(&local_entity)
                }) {
                    debug!(
                        "insert fake Predicted on {:?}. PredictionTarget: {:?}",
                        entity, d.prediction
                    );
                    commands.entity(entity).insert(Predicted);
                }

                #[cfg(feature = "interpolation")]
                if d.interpolation.as_ref().is_some_and(|p| {
                    (p.is_added() || host_client.is_added()) && p.senders.contains(&local_entity)
                }) {
                    commands.entity(entity).insert(Interpolated);
                }
            };

        query.iter().for_each(|d| {
            add_fake_components(&mut commands, &d, d.entity);
            if let Some(replicate_like) = &d.replicate_like
                && replicate_like.is_changed()
                && let Ok(root_d) = query.get(replicate_like.root)
            {
                // TODO: when a component changes on the root, we should also check if we need to add
                //  fake components on the children
                add_fake_components(&mut commands, &root_d, d.entity);
            }
        });
    }
}

impl Plugin for HostServerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PostUpdate, Self::add_prediction_interpolation_components);
    }
}
