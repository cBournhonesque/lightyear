//! Module related to the concept of `Authority`
//!
//! A peer is said to have authority over an entity if it has the burden of simulating the entity.
//! Note that replicating state to other peers doesn't necessary mean that you have authority:
//! client C1 could have authority (is simulating the entity), replicated to the server which then replicates to other clients.
//! In this case C1 has authority even though the server is still replicating some states.
//!

use crate::prelude::{ClientId, Deserialize, Serialize};
use bevy::ecs::entity::MapEntities;
use bevy::prelude::*;

/// Authority is used to define who is in charge of simulating an entity.
///
/// In particular:
/// - a client with Authority won't accept replication updates from the server
/// - a client without Authority won't be sending any replication updates
/// - a server won't accept replication updates from clients without Authority
#[derive(Component, Debug, Default, Clone, Copy, PartialEq, Eq, Reflect)]
#[reflect(Component)]
pub struct HasAuthority;

#[derive(
    Component, Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default, Reflect,
)]
#[reflect(Component)]
pub enum AuthorityPeer {
    None,
    #[default]
    Server,
    Client(ClientId),
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AuthorityChange {
    pub entity: Entity,
    pub gain_authority: bool,
    /// Should we add prediction for that entity? This can be useful if the entity was originally
    /// spawned by a client C1, and then the authority was transferred away from that client.
    /// Now we want to start predicting the entity on client C1, but we cannot just rely on the normal
    /// systems because the entity already exists, and ShouldBePredicted only gets sent on the initial Spawn message
    pub add_prediction: bool,
    pub add_interpolation: bool,
}

impl MapEntities for AuthorityChange {
    fn map_entities<M: EntityMapper>(&mut self, entity_mapper: &mut M) {
        self.entity = entity_mapper.get_mapped(self.entity);
    }
}

#[cfg(test)]
mod tests {
    use crate::client::prediction::predicted_history::PredictionHistory;
    use crate::prelude::client::{Confirmed, ConfirmedHistory};
    use crate::prelude::server::{Replicate, SyncTarget};
    use crate::prelude::{client, server, ClientId, NetworkTarget, Replicated};
    use crate::server::replication::commands::AuthorityCommandExt;
    use crate::shared::replication::authority::{AuthorityPeer, HasAuthority};
    use crate::tests::multi_stepper::{MultiBevyStepper, TEST_CLIENT_ID_1, TEST_CLIENT_ID_2};
    use crate::tests::protocol::{
        ComponentMapEntities, ComponentSyncModeFull, ComponentSyncModeSimple,
    };
    use crate::tests::stepper::{BevyStepper, TEST_CLIENT_ID};
    use bevy::prelude::{default, Entity};

    #[test]
    fn test_transfer_authority_server_to_client() {
        // tracing_subscriber::FmtSubscriber::builder()
        //     .with_max_level(tracing::Level::ERROR)
        //     .init();
        let mut stepper = BevyStepper::default();

        let server_entity = stepper
            .server_app
            .world_mut()
            .spawn((server::Replicate::default(), ComponentSyncModeSimple(1.0)))
            .id();

        stepper.flush();
        // check that HasAuthority was added
        assert!(stepper
            .server_app
            .world()
            .get::<HasAuthority>(server_entity)
            .is_some());

        stepper.frame_step();
        stepper.frame_step();
        // check that the entity was replicated
        let client_entity = stepper
            .client_app
            .world()
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_entity)
            .expect("entity was not replicated to client");
        assert_eq!(
            stepper
                .client_app
                .world()
                .get::<Replicated>(client_entity)
                .unwrap(),
            &Replicated { from: None }
        );

        // transfer authority from server to client
        // NOTE: we NEED to add Replicate before transferring authority
        stepper
            .client_app
            .world_mut()
            .entity_mut(client_entity)
            .insert(client::Replicate::default())
            // don't add authority directly
            .remove::<HasAuthority>();
        stepper
            .server_app
            .world_mut()
            .commands()
            .entity(server_entity)
            .transfer_authority(AuthorityPeer::Client(ClientId::Netcode(TEST_CLIENT_ID)));

        // check that the authority was transferred correctly
        stepper.flush();
        assert!(stepper
            .server_app
            .world()
            .get::<HasAuthority>(server_entity)
            .is_none());
        assert_eq!(
            stepper
                .server_app
                .world()
                .get::<Replicated>(server_entity)
                .unwrap(),
            &Replicated {
                from: Some(ClientId::Netcode(TEST_CLIENT_ID))
            }
        );
        assert_eq!(
            stepper
                .server_app
                .world()
                .get::<AuthorityPeer>(server_entity)
                .unwrap(),
            &AuthorityPeer::Client(ClientId::Netcode(TEST_CLIENT_ID))
        );
        stepper.frame_step();
        stepper.frame_step();
        stepper.flush();
        assert!(stepper
            .client_app
            .world()
            .get::<HasAuthority>(client_entity)
            .is_some());
        assert!(stepper
            .client_app
            .world()
            .get::<Replicated>(client_entity)
            .is_none());

        // transfer authority from client to None
        stepper
            .server_app
            .world_mut()
            .commands()
            .entity(server_entity)
            .transfer_authority(AuthorityPeer::None);
        stepper.flush();
        stepper.frame_step();
        stepper.frame_step();
        stepper.flush();
        assert!(stepper
            .server_app
            .world()
            .get::<Replicated>(server_entity)
            .is_none());
        assert!(stepper
            .client_app
            .world()
            .get::<HasAuthority>(client_entity)
            .is_none());
        // TODO: currently, if the client loses authority, we re-add Replicated with from: None
        //  to symbolize that it is receiving replication update. Maybe we need to handle the edge-case
        //  where there is no authority, and we just remove Replicated entirely!
        //  Fix this only if this comes up
        // assert!(stepper
        //     .client_app
        //     .world()
        //     .get::<Replicated>(client_entity)
        //     .is_none());
    }

    /// Spawn on client, transfer authority to server, despawn entity on server.
    /// The entity should get despawned correctly on client.
    /// Relevant issue: https://github.com/cBournhonesque/lightyear/issues/644
    #[test]
    fn test_transfer_authority_client_to_server_despawn() {
        // tracing_subscriber::FmtSubscriber::builder()
        //     .with_max_level(tracing::Level::ERROR)
        //     .init();
        let mut stepper = BevyStepper::default();

        let client_entity = stepper
            .client_app
            .world_mut()
            .spawn(client::Replicate::default())
            .id();

        // TODO: we need to run a couple frames because the server doesn't read the client's updates
        //  because they are from the future
        for _ in 0..10 {
            stepper.frame_step();
            stepper.frame_step();
        }

        // check that the entity was replicated
        let server_entity = stepper
            .server_app
            .world()
            .resource::<server::ConnectionManager>()
            .connection(ClientId::Netcode(TEST_CLIENT_ID))
            .expect("client connection missing")
            .replication_receiver
            .remote_entity_map
            .get_local(client_entity)
            .expect("entity was not replicated to server");
        assert_eq!(
            stepper
                .server_app
                .world()
                .get::<Replicated>(server_entity)
                .unwrap(),
            &Replicated {
                from: Some(ClientId::Netcode(TEST_CLIENT_ID))
            }
        );

        // add Replicate to the entity to mark it for replication
        // TODO: resolve this footgun
        // IMPORTANT: we need to do this BEFORE transferring authority
        //  or we will be replicating a Spawn message
        stepper
            .server_app
            .world_mut()
            .entity_mut(server_entity)
            .insert(server::Replicate {
                authority: AuthorityPeer::Client(ClientId::Netcode(TEST_CLIENT_ID)),
                ..default()
            });
        // transfer authority from client to server
        stepper
            .server_app
            .world_mut()
            .commands()
            .entity(server_entity)
            .transfer_authority(AuthorityPeer::Server);
        stepper.frame_step();
        stepper.frame_step();
        assert!(stepper
            .server_app
            .world()
            .get::<HasAuthority>(server_entity)
            .is_some());
        assert!(stepper
            .client_app
            .world()
            .get::<HasAuthority>(client_entity)
            .is_none());

        // despawn entity on server
        stepper.server_app.world_mut().despawn(server_entity);
        stepper.frame_step();
        stepper.frame_step();

        // check that the entity was despawned on the client
        assert!(stepper
            .client_app
            .world()
            .entities()
            .get(client_entity)
            .is_none());
    }

    #[test]
    fn test_ignore_updates_from_non_authority() {
        let mut stepper = BevyStepper::default();

        let client_entity = stepper
            .client_app
            .world_mut()
            .spawn((client::Replicate::default(), ComponentSyncModeFull(1.0)))
            .id();

        // TODO: we need to run a couple frames because the server doesn't read the client's updates
        //  because they are from the future
        for _ in 0..10 {
            stepper.frame_step();
            stepper.frame_step();
        }
        // check that the entity was replicated
        let server_entity = stepper
            .server_app
            .world()
            .resource::<server::ConnectionManager>()
            .connection(ClientId::Netcode(TEST_CLIENT_ID))
            .expect("client connection missing")
            .replication_receiver
            .remote_entity_map
            .get_local(client_entity)
            .expect("entity was not replicated to server");
        assert_eq!(
            stepper
                .server_app
                .world()
                .get::<Replicated>(server_entity)
                .unwrap(),
            &Replicated {
                from: Some(ClientId::Netcode(TEST_CLIENT_ID))
            }
        );

        // add Replicate to the entity to mark it for replication
        // TODO: resolve this footgun
        // IMPORTANT: we need to do this BEFORE transferring authority
        //  or we will be replicating a Spawn message
        stepper
            .server_app
            .world_mut()
            .entity_mut(server_entity)
            .insert(server::Replicate {
                authority: AuthorityPeer::Client(ClientId::Netcode(TEST_CLIENT_ID)),
                ..default()
            });
        // transfer authority from client to server
        stepper
            .server_app
            .world_mut()
            .commands()
            .entity(server_entity)
            .transfer_authority(AuthorityPeer::Server);

        // check that the authority was transferred correctly
        stepper.flush();
        assert!(stepper
            .server_app
            .world()
            .get::<HasAuthority>(server_entity)
            .is_some());
        assert!(stepper
            .server_app
            .world()
            .get::<Replicated>(server_entity)
            .is_none());
        assert_eq!(
            stepper
                .server_app
                .world()
                .get::<AuthorityPeer>(server_entity)
                .unwrap(),
            &AuthorityPeer::Server
        );
        stepper.frame_step();
        stepper.frame_step();
        stepper.flush();
        assert!(stepper
            .client_app
            .world()
            .get::<HasAuthority>(client_entity)
            .is_none());
        assert_eq!(
            stepper
                .client_app
                .world()
                .get::<Replicated>(client_entity)
                .unwrap(),
            &Replicated { from: None }
        );

        // create a conflict situation where the client also gets added `HasAuthority` at the same time as the server (which is what happens when the AuthorityChange message is in flight)
        // TODO: it does mean that server changes while the client is in the process of getting authority are ignored. Is this what we want? Maybe we make the client always accept remote changes?
        stepper
            .client_app
            .world_mut()
            .entity_mut(client_entity)
            .insert(HasAuthority);
        // update the component
        stepper
            .client_app
            .world_mut()
            .get_mut::<ComponentSyncModeFull>(client_entity)
            .unwrap()
            .0 = 2.0;
        for _ in 0..10 {
            stepper.frame_step();
            stepper.frame_step();
        }
        // check that the server didn't accept the client's update because it believes that it has the authority (AuthorityPeer = Server)
        assert_eq!(
            stepper
                .server_app
                .world()
                .get::<ComponentSyncModeFull>(server_entity)
                .unwrap()
                .0,
            1.0
        );
    }

    /// Spawn on client, transfer authority to server
    /// Update on server, the updates from the server use entity mapping on the send side.
    /// (both for the Entity in Updates and for the content of the components in the Update)
    #[test]
    fn test_receive_updates_from_transferred_authority_client_to_server() {
        let mut stepper = BevyStepper::default();

        let client_entity_1 = stepper
            .client_app
            .world_mut()
            .spawn(client::Replicate::default())
            .id();
        let client_entity_2 = stepper
            .client_app
            .world_mut()
            .spawn((
                client::Replicate::default(),
                ComponentMapEntities(Entity::PLACEHOLDER),
            ))
            .id();

        // TODO: we need to run a couple frames because the server doesn't read the client's updates
        //  because they are from the future
        for _ in 0..10 {
            stepper.frame_step();
            stepper.frame_step();
        }
        // check that the entity was replicated
        let server_entity_1 = stepper
            .server_app
            .world()
            .resource::<server::ConnectionManager>()
            .connection(ClientId::Netcode(TEST_CLIENT_ID))
            .expect("client connection missing")
            .replication_receiver
            .remote_entity_map
            .get_local(client_entity_1)
            .expect("entity was not replicated to server");
        // check that the entity was replicated
        let server_entity_2 = stepper
            .server_app
            .world()
            .resource::<server::ConnectionManager>()
            .connection(ClientId::Netcode(TEST_CLIENT_ID))
            .expect("client connection missing")
            .replication_receiver
            .remote_entity_map
            .get_local(client_entity_2)
            .expect("entity was not replicated to server");

        // add Replicate to the entity to mark it for replication
        // TODO: resolve this footgun
        // IMPORTANT: we need to do this BEFORE transferring authority
        //  or we will be replicating a Spawn message
        stepper
            .server_app
            .world_mut()
            .entity_mut(server_entity_2)
            .insert(server::Replicate {
                authority: AuthorityPeer::Client(ClientId::Netcode(TEST_CLIENT_ID)),
                ..default()
            });

        // transfer authority from server to client
        stepper
            .server_app
            .world_mut()
            .commands()
            .entity(server_entity_2)
            .transfer_authority(AuthorityPeer::Server);

        // check that the authority was transferred correctly
        stepper.flush();
        assert!(stepper
            .server_app
            .world()
            .get::<HasAuthority>(server_entity_2)
            .is_some());
        assert_eq!(
            stepper
                .server_app
                .world()
                .get::<AuthorityPeer>(server_entity_2)
                .unwrap(),
            &AuthorityPeer::Server
        );
        stepper.frame_step();
        stepper.frame_step();
        stepper.flush();
        assert!(stepper
            .client_app
            .world()
            .get::<HasAuthority>(client_entity_2)
            .is_none());

        // update the component on the server.
        // the EntityActions should be mapped from server_entity_2 to client_entity_2
        // the ComponentMapEntities.0 should be mapped from server_entity_1 to client_entity_1
        stepper
            .server_app
            .world_mut()
            .get_mut::<ComponentMapEntities>(server_entity_2)
            .unwrap()
            .0 = server_entity_1;
        for _ in 0..10 {
            stepper.frame_step();
        }

        assert_eq!(
            stepper
                .client_app
                .world()
                .get::<ComponentMapEntities>(client_entity_2)
                .unwrap()
                .0,
            client_entity_1
        );
    }

    /// Spawn on client, transfer authority from client 1 to client 2
    /// Update on server, the updates from the server use entity mapping on the send side.
    /// (both for the Entity in Updates and for the content of the components in the Update)
    #[test]
    fn test_receive_updates_from_transferred_authority_client_to_client() {
        // tracing_subscriber::FmtSubscriber::builder()
        //     .with_max_level(tracing::Level::ERROR)
        //     .init();
        let mut stepper = MultiBevyStepper::default();

        let client_entity_1a = stepper
            .client_app_1
            .world_mut()
            .spawn(client::Replicate::default())
            .id();
        let client_entity_1b = stepper
            .client_app_1
            .world_mut()
            .spawn((
                client::Replicate::default(),
                ComponentMapEntities(Entity::PLACEHOLDER),
            ))
            .id();

        // TODO: we need to run a couple frames because the server doesn't read the client's updates
        //  because they are from the future
        for _ in 0..10 {
            stepper.frame_step();
            stepper.frame_step();
        }
        // check that the entity was replicated
        let server_entity_a = stepper
            .server_app
            .world()
            .resource::<server::ConnectionManager>()
            .connection(ClientId::Netcode(TEST_CLIENT_ID_1))
            .expect("client connection missing")
            .replication_receiver
            .remote_entity_map
            .get_local(client_entity_1a)
            .expect("entity was not replicated to server");
        // check that the entity was replicated
        let server_entity_b = stepper
            .server_app
            .world()
            .resource::<server::ConnectionManager>()
            .connection(ClientId::Netcode(TEST_CLIENT_ID_1))
            .expect("client connection missing")
            .replication_receiver
            .remote_entity_map
            .get_local(client_entity_1b)
            .expect("entity was not replicated to server");

        // add Replicate to the entity to mark it for replication
        // TODO: resolve this footgun
        // IMPORTANT: we need to do this BEFORE transferring authority
        //  or we will be replicating a Spawn message. Right now we don't
        //  because
        //  - we don't send spawn to the original client that spawned the entity
        //  - we don't send spawn to the client that has authority
        stepper
            .server_app
            .world_mut()
            .entity_mut(server_entity_a)
            .insert(server::Replicate {
                authority: AuthorityPeer::Client(ClientId::Netcode(TEST_CLIENT_ID_1)),
                ..default()
            });
        stepper
            .server_app
            .world_mut()
            .entity_mut(server_entity_b)
            .insert(server::Replicate {
                authority: AuthorityPeer::Client(ClientId::Netcode(TEST_CLIENT_ID_1)),
                ..default()
            });

        stepper.frame_step();
        stepper.frame_step();

        // check that the entity was spawned on the other client
        let client_entity_2a = stepper
            .client_app_2
            .world()
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_entity_a)
            .expect("entity was not replicated to server");
        // check that the entity was replicated
        let client_entity_2b = stepper
            .client_app_2
            .world()
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_entity_b)
            .expect("entity was not replicated to server");

        // TODO: resolve this footgun
        // add replicate BEFORE we transfer authority (otherwise when we add client::Replicate, it would add HasAuthority with it...)
        stepper
            .client_app_2
            .world_mut()
            .entity_mut(client_entity_2b)
            .insert(client::Replicate::default())
            .remove::<HasAuthority>();

        // transfer authority from client1 to client2
        stepper
            .server_app
            .world_mut()
            .commands()
            .entity(server_entity_b)
            .transfer_authority(AuthorityPeer::Client(ClientId::Netcode(TEST_CLIENT_ID_2)));

        // check that the authority was transferred correctly
        stepper.flush();
        assert!(stepper
            .server_app
            .world()
            .get::<HasAuthority>(server_entity_b)
            .is_none());
        assert_eq!(
            stepper
                .server_app
                .world()
                .get::<Replicated>(server_entity_b)
                .unwrap(),
            &Replicated {
                from: Some(ClientId::Netcode(TEST_CLIENT_ID_2))
            }
        );
        assert_eq!(
            stepper
                .server_app
                .world()
                .get::<AuthorityPeer>(server_entity_b)
                .unwrap(),
            &AuthorityPeer::Client(ClientId::Netcode(TEST_CLIENT_ID_2))
        );
        stepper.frame_step();
        stepper.frame_step();
        stepper.flush();
        assert!(stepper
            .client_app_1
            .world()
            .get::<HasAuthority>(client_entity_1b)
            .is_none());
        assert_eq!(
            stepper
                .client_app_1
                .world()
                .get::<Replicated>(client_entity_1b)
                .unwrap(),
            &Replicated { from: None }
        );
        assert!(stepper
            .client_app_2
            .world()
            .get::<Replicated>(client_entity_2b)
            .is_none());

        // update the component on the client.
        // the EntityActions should be mapped:
        // - from client_entity_2b to server_entity_b when sending to server
        // - from server_entity_b to client_entity_1b when server sends to client_1
        // the ComponentMapEntities.0 should be mapped:
        // - from client_entity_2a to server_entity_a when sending to server
        // - from server_entity_a to client_entity_1a when server sends to client_1
        stepper
            .client_app_2
            .world_mut()
            .get_mut::<ComponentMapEntities>(client_entity_2b)
            .unwrap()
            .0 = client_entity_2a;
        for _ in 0..10 {
            stepper.frame_step();
        }

        assert_eq!(
            stepper
                .server_app
                .world()
                .get::<ComponentMapEntities>(server_entity_b)
                .unwrap()
                .0,
            server_entity_a
        );
        // TODO: this probably doesn't work because the update is received AFTER the authority-peer change
        //  so the update is still sent as an update, not as an insert. What we want is:
        //  the first update ever sent to a client is an insert.
        assert_eq!(
            stepper
                .client_app_1
                .world()
                .get::<ComponentMapEntities>(client_entity_1b)
                .unwrap()
                .0,
            client_entity_1a
        );
    }

    // Check for https://github.com/cBournhonesque/lightyear/issues/639
    // - Spawn an entity on C1 and replicate to server
    // - Server replicates to all with Interpolation enabled
    // - Transfer authority from C1 to C2
    // Check that:
    // - C1 spawns an interpolated entity
    // - The ConfirmedTick gets updated correctly on C1 when the component is updated on C2
    // - a ConfirmedHistory buffer is created on C1
    // Bonus:
    // - Server doesn't replicate all components back to C1 when the authority switches to C2
    // - C2 doesn't replicate all components back to S when the authority switches to C2
    // These 2 only work because there is a gap between adding the Replicate component on S or C2
    // and transferring authority
    #[test]
    fn test_transfer_authority_with_interpolation() {
        // tracing_subscriber::FmtSubscriber::builder()
        //     .with_max_level(tracing::Level::ERROR)
        //     .init();
        let mut stepper = MultiBevyStepper::default();
        let client_entity_1 = stepper
            .client_app_1
            .world_mut()
            .spawn((client::Replicate::default(), ComponentSyncModeFull(1.0)))
            .id();
        // TODO: we need to run a couple frames because the server doesn't read the client's updates
        //  because they are from the future
        for _ in 0..10 {
            stepper.frame_step();
        }
        let server_entity = stepper
            .server_app
            .world()
            .resource::<server::ConnectionManager>()
            .connection(ClientId::Netcode(TEST_CLIENT_ID_1))
            .expect("client connection missing")
            .replication_receiver
            .remote_entity_map
            .get_local(client_entity_1)
            .expect("entity was not replicated to server");

        // Add replicate on the server, with interpolation
        // NOTE: this has to happen BEFORE we transfer authority
        stepper
            .server_app
            .world_mut()
            .entity_mut(server_entity)
            .insert(Replicate {
                authority: AuthorityPeer::Client(ClientId::Netcode(TEST_CLIENT_ID_1)),
                sync: SyncTarget {
                    prediction: NetworkTarget::None,
                    interpolation: NetworkTarget::All,
                },
                ..default()
            });
        stepper.frame_step();
        stepper.frame_step();

        // entity was replicated to client 2
        let client_entity_2 = stepper
            .client_app_2
            .world()
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_entity)
            .expect("entity was not replicated to server");
        // check that it has confirmed and interpolated
        let confirmed_2 = stepper
            .client_app_2
            .world()
            .get::<Confirmed>(client_entity_2)
            .expect("confirmed missing on client 2");
        let interpolated_2 = confirmed_2
            .interpolated
            .expect("interpolated entity missing on client 2");

        // add Replicate on it as well, but without switching authority
        stepper
            .client_app_2
            .world_mut()
            .entity_mut(client_entity_2)
            .insert(client::Replicate::default())
            .remove::<HasAuthority>();

        // TODO: even without those 2 frame_step, i.e. if adding Replicate is done at the same time as transferring authority
        //  then the new authoritative peer (client 2) won't replicate all components to the server
        //  because the authority change message takes a bit of time to arrive
        //  How can we test this?
        // stepper.frame_step();
        // stepper.frame_step();

        // transfer authority from client 1 to client 2
        stepper
            .server_app
            .world_mut()
            .commands()
            .entity(server_entity)
            .transfer_authority(AuthorityPeer::Client(ClientId::Netcode(TEST_CLIENT_ID_2)));

        // advance frames.
        for _ in 0..10 {
            stepper.frame_step();
        }

        // confirm that an interpolated entity is spawned on the client 1 that lost authority
        let confirmed_1 = stepper
            .client_app_1
            .world()
            .get::<Confirmed>(client_entity_1)
            .expect("confirmed missing on client 1");
        let interpolated_1 = confirmed_1
            .interpolated
            .expect("interpolated entity missing on client 1");

        stepper.frame_step();
        stepper.frame_step();

        // confirm that interpolation is working correctly by updating the component on the client 2
        stepper
            .client_app_2
            .world_mut()
            .get_mut::<ComponentSyncModeFull>(client_entity_2)
            .unwrap()
            .0 = 2.0;

        // give enough time for replication + interpolation
        for _ in 0..10 {
            stepper.frame_step();
        }

        // TODO: find a way to test that the confirmed tick has been updated
        //  and that the component has been added to the ConfirmedHistory buffer
        // confirm that the interpolated entity has the updated value on both clients
        // it's hard to confirm this
        assert_eq!(
            stepper
                .client_app_2
                .world()
                .get::<ComponentSyncModeFull>(interpolated_2)
                .unwrap()
                .0,
            2.0
        );
        assert!(stepper
            .client_app_1
            .world()
            .get::<ConfirmedHistory<ComponentSyncModeFull>>(interpolated_2)
            .is_some());

        // This fails for some reason, maybe because we need more updates for the interpolation to take effect?
        // assert_eq!(
        //     stepper
        //         .client_app_1
        //         .world()
        //         .get::<ComponentSyncModeFull>(interpolated_1)
        //         .unwrap()
        //         .0,
        //     2.0
        // );
    }

    // Check for https://github.com/cBournhonesque/lightyear/issues/639
    // - Spawn an entity on C1 and replicate to server
    // - Server replicates to all with Prediction enabled
    // - Transfer authority from C1 to Server
    // Check that:
    // - C1 spawns an predicted entity
    // - The ConfirmedTick gets updated correctly on C1 when the component is updated on server
    // - a PredictionHistory buffer is created on C1
    // Bonus:
    // - Server doesn't replicate all components back to C1 when the authority switches to S
    #[test]
    fn test_transfer_authority_with_prediction() {
        // tracing_subscriber::FmtSubscriber::builder()
        //     .with_max_level(tracing::Level::ERROR)
        //     .init();
        let mut stepper = MultiBevyStepper::default();
        let client_entity_1 = stepper
            .client_app_1
            .world_mut()
            .spawn((client::Replicate::default(), ComponentSyncModeFull(1.0)))
            .id();
        // TODO: we need to run a couple frames because the server doesn't read the client's updates
        //  because they are from the future
        for _ in 0..10 {
            stepper.frame_step();
        }
        let server_entity = stepper
            .server_app
            .world()
            .resource::<server::ConnectionManager>()
            .connection(ClientId::Netcode(TEST_CLIENT_ID_1))
            .expect("client connection missing")
            .replication_receiver
            .remote_entity_map
            .get_local(client_entity_1)
            .expect("entity was not replicated to server");

        // Add replicate on the server, with prediction
        // NOTE: this has to happen BEFORE we transfer authority
        stepper
            .server_app
            .world_mut()
            .entity_mut(server_entity)
            .insert(Replicate {
                authority: AuthorityPeer::Client(ClientId::Netcode(TEST_CLIENT_ID_1)),
                sync: SyncTarget {
                    prediction: NetworkTarget::All,
                    interpolation: NetworkTarget::None,
                },
                ..default()
            });
        stepper.frame_step();
        stepper.frame_step();

        // entity was replicated to client 2
        let client_entity_2 = stepper
            .client_app_2
            .world()
            .resource::<client::ConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_entity)
            .expect("entity was not replicated to server");
        // check that it has confirmed and interpolated
        let confirmed_2 = stepper
            .client_app_2
            .world()
            .get::<Confirmed>(client_entity_2)
            .expect("confirmed missing on client 2");
        let predicted_2 = confirmed_2
            .predicted
            .expect("predicted entity missing on client 2");

        // transfer authority from client 1 to server
        stepper
            .server_app
            .world_mut()
            .commands()
            .entity(server_entity)
            .transfer_authority(AuthorityPeer::Server);

        // advance frames.
        for _ in 0..10 {
            stepper.frame_step();
        }

        // confirm that a predicted entity is spawned on the client 1 that lost authority
        let confirmed_1 = stepper
            .client_app_1
            .world()
            .get::<Confirmed>(client_entity_1)
            .expect("confirmed missing on client 1");
        let predicted_1 = confirmed_1
            .predicted
            .expect("prediced entity missing on client 1");

        stepper.frame_step();
        stepper.frame_step();

        // confirm that prediction is working correctly by updating the component on the server
        stepper
            .server_app
            .world_mut()
            .get_mut::<ComponentSyncModeFull>(server_entity)
            .unwrap()
            .0 = 2.0;

        // give enough time for replication + prediction
        for _ in 0..10 {
            stepper.frame_step();
        }

        // TODO: find a way to test that the confirmed tick has been updated
        //  and that the component has been added to the PredictionHistory buffer
        assert_eq!(
            stepper
                .client_app_2
                .world()
                .get::<ComponentSyncModeFull>(predicted_2)
                .unwrap()
                .0,
            2.0
        );
        assert!(stepper
            .client_app_1
            .world()
            .get::<PredictionHistory<ComponentSyncModeFull>>(predicted_1)
            .is_some());
        assert_eq!(
            stepper
                .client_app_1
                .world()
                .get::<ComponentSyncModeFull>(predicted_1)
                .unwrap()
                .0,
            2.0
        );
    }
}
