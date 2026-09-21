use crate::stepper::*;
use lightyear::prelude::client::*;
use lightyear::prelude::*;
use lightyear_connection::server::{Stop, Stopped};
use test_log::test;

/// Exercises the same stepper and connection stack as the other client/server tests, but with a
/// real local WebTransport session. The OS-selected port makes this safe to run alongside other
/// tests without maintaining a separate WebTransport harness.
#[cfg(all(feature = "webtransport", not(target_family = "wasm")))]
#[test]
fn test_setup_netcode_webtransport_client_server() {
    let mut stepper =
        ClientServerStepper::from_config(StepperConfig::single().with_io(IoType::WebTransport));

    assert_ne!(stepper.server_addr.port(), 0);
    assert_eq!(
        stepper.server().get::<WebTransportLocalAddr>().unwrap().0,
        stepper.server_addr
    );
    let peer = stepper.client(0).get::<WebTransportPeerAddr>().unwrap().0;
    assert_eq!(peer.ip().to_canonical(), stepper.server_addr.ip());
    assert_eq!(peer.port(), stepper.server_addr.port());
    assert!(stepper.client(0).contains::<Connected>());
    assert!(stepper.client_of(0).contains::<Connected>());
    assert!(input_timeline_is_synced(stepper.client_app().world()));
    assert!(interpolation_timeline_is_synced(
        stepper.client_app().world()
    ));
}

#[cfg(all(feature = "webtransport", not(target_family = "wasm")))]
mod identity_webtransport {
    use super::*;
    use crate::protocol::{Channel1, StringMessage};
    use bevy::prelude::*;
    use lightyear::prelude::server::{NetcodeServer, WebTransportEndpoint};
    use lightyear_connection::network_topology::NetworkingMetadata;
    use lightyear_netcode::server_plugin::NetcodeServerError;
    use lightyear_netcode::{ConnectToken, IdentityBinding};
    use std::time::Duration;
    use test_log::test;

    #[derive(Resource, Default)]
    struct ReceivedMessages(Vec<(Entity, StringMessage)>);

    fn identity_server() -> (ClientServerStepper, [u8; 32]) {
        let mut stepper = ClientServerStepper::from_config(
            StepperConfig::with_netcode_clients(0).with_io(IoType::WebTransport),
        );
        stepper
            .server_app
            .set_error_handler(|error, _| panic!("identity server lifecycle failed: {error}"));
        let hash = stepper
            .server()
            .get::<WebTransportEndpoint>()
            .unwrap()
            .certificate
            .certificate_chain()
            .as_slice()[0]
            .hash();
        let digest = *hash.as_ref();
        let binding = IdentityBinding::from_native(b"webtransport-cert-sha256", &digest);
        stepper
            .server_mut()
            .get_mut::<NetcodeServer>()
            .unwrap()
            .set_identity_binding(binding);
        (stepper, digest)
    }

    fn identity_client(stepper: &mut ClientServerStepper, token_digest: [u8; 32]) {
        let client = stepper.new_client(ClientType::Netcode, None);
        stepper.client_apps[client]
            .set_error_handler(|error, _| panic!("identity client lifecycle failed: {error}"));
        let binding = IdentityBinding::from_native(b"webtransport-cert-sha256", &token_digest);
        let bytes = ConnectToken::build_identity(binding, 0, client as u64 + 1, [0; 32])
            .generate()
            .unwrap()
            .try_into_bytes()
            .unwrap();
        let token = ConnectToken::try_from_bytes(&bytes).unwrap();
        let target = format!(
            "https://127.0.0.1:{}/identity?authentication=netcode",
            stepper.server_addr.port()
        );
        let mut entity = stepper.client_mut(client);
        let mut transport = entity.get_mut::<WebTransportClientIo>().unwrap();
        // The URL is caller-owned: an identity token supplies no socket dial target.
        transport.target = target;
        entity.insert(
            NetcodeClient::new(
                Authentication::Token(token),
                lightyear_netcode::client_plugin::NetcodeConfig::default(),
            )
            .unwrap(),
        );
    }

    #[test]
    fn test_identity_token_webtransport_message_and_cleanup() {
        let (mut stepper, digest) = identity_server();
        identity_client(&mut stepper, digest);
        stepper.server_app.init_resource::<ReceivedMessages>();
        stepper.server_app.add_systems(
            Update,
            |mut receivers: Query<(Entity, &mut MessageReceiver<StringMessage>)>,
             mut received: ResMut<ReceivedMessages>| {
                for (entity, mut receiver) in &mut receivers {
                    received
                        .0
                        .extend(receiver.receive().map(|message| (entity, message)));
                }
            },
        );
        stepper.init();
        assert!(stepper.client(0).contains::<Connected>());
        assert!(stepper.client_of(0).contains::<Connected>());
        let accepted = stepper.client_of_entities[0];
        assert_eq!(
            stepper.client_of(0).get::<RemoteId>(),
            Some(&RemoteId(PeerId(1)))
        );
        assert_eq!(
            stepper
                .server_app
                .world()
                .resource::<NetworkingMetadata>()
                .peer_map
                .get(&PeerId(1)),
            Some(&accepted),
        );

        let message = StringMessage("authenticated over pinned WebTransport".to_owned());
        stepper
            .client_mut(0)
            .get_mut::<MessageSender<StringMessage>>()
            .unwrap()
            .send::<Channel1>(message.clone());
        for _ in 0..400 {
            stepper.frame_step(1);
            if !stepper
                .server_app
                .world()
                .resource::<ReceivedMessages>()
                .0
                .is_empty()
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            stepper.server_app.world().resource::<ReceivedMessages>().0,
            [(accepted, message)],
        );

        let client = stepper.client_entities[0];
        stepper.client_apps[0]
            .world_mut()
            .trigger(Disconnect { entity: client });
        for _ in 0..400 {
            stepper.frame_step(1);
            if stepper.server_app.world().get_entity(accepted).is_err()
                && stepper.client(0).contains::<Unlinked>()
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        // Also exercise frames after retirement, where a stale sender or route would survive.
        stepper.frame_step(5);
        assert!(stepper.server_app.world().get_entity(accepted).is_err());
        assert!(stepper.client(0).contains::<Unlinked>());
        assert!(!stepper.client(0).contains::<Connected>());
        assert!(
            !stepper
                .server_app
                .world()
                .resource::<NetworkingMetadata>()
                .peer_map
                .contains_key(&PeerId(1))
        );
        assert!(
            !stepper.client_apps[0]
                .world()
                .resource::<NetworkingMetadata>()
                .peer_map
                .contains_key(&PeerId::SERVER)
        );
    }

    #[test]
    fn test_identity_token_webtransport_rejects_wrong_binding() {
        #[derive(Resource, Default)]
        struct BindingRejections(usize);

        let (mut stepper, digest) = identity_server();
        stepper.server_app.init_resource::<BindingRejections>();
        stepper.server_app.add_systems(
            Update,
            |mut errors: MessageReader<NetcodeServerError>,
             mut rejections: ResMut<BindingRejections>| {
                for error in errors.read() {
                    if matches!(
                        &error.error,
                        lightyear_netcode::Error::IdentityTokenBindingMismatch
                    ) {
                        rejections.0 += 1;
                    }
                }
            },
        );
        let mut wrong_digest = digest;
        wrong_digest[0] ^= 1;
        // TLS trusts the actual endpoint, but the signed token authorizes another certificate.
        identity_client(&mut stepper, wrong_digest);
        let client = stepper.client_entities[0];
        stepper.client_apps[0]
            .world_mut()
            .trigger(Connect { entity: client });
        for _ in 0..400 {
            stepper.frame_step(1);
            if stepper.server_app.world().resource::<BindingRejections>().0 != 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(stepper.server_app.world().resource::<BindingRejections>().0 > 0);
        assert!(stepper.client(0).contains::<Linked>());
        assert!(!stepper.client(0).contains::<Connected>());
        assert!(
            !stepper
                .server_app
                .world()
                .resource::<NetworkingMetadata>()
                .peer_map
                .contains_key(&PeerId(1))
        );
        assert!(
            !stepper.client_apps[0]
                .world()
                .resource::<NetworkingMetadata>()
                .peer_map
                .contains_key(&PeerId::SERVER)
        );
        stepper.client_apps[0]
            .world_mut()
            .trigger(Disconnect { entity: client });
    }

    #[test]
    fn test_identity_token_webtransport_rejects_wrong_certificate_pin() {
        let (mut stepper, digest) = identity_server();
        identity_client(&mut stepper, digest);
        // The token authorizes the real endpoint, but the client's trusted native identity differs.
        stepper
            .client_mut(0)
            .get_mut::<WebTransportClientIo>()
            .unwrap()
            .certificate_digest
            .replace_range(..2, &format!("{:02x}", digest[0] ^ 1));
        let client = stepper.client_entities[0];
        stepper.client_apps[0]
            .world_mut()
            .trigger(Connect { entity: client });
        for _ in 0..400 {
            stepper.frame_step(1);
            if stepper
                .client(0)
                .get::<Unlinked>()
                .is_some_and(|state| matches!(&state.reason, UnlinkReason::TransportError(_)))
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            stepper
                .client(0)
                .get::<Unlinked>()
                .is_some_and(|state| { matches!(&state.reason, UnlinkReason::TransportError(_)) })
        );
        assert!(!stepper.client(0).contains::<Connected>());
        assert!(
            !stepper
                .server_app
                .world()
                .resource::<NetworkingMetadata>()
                .peer_map
                .contains_key(&PeerId(1))
        );
        assert!(
            !stepper.client_apps[0]
                .world()
                .resource::<NetworkingMetadata>()
                .peer_map
                .contains_key(&PeerId::SERVER)
        );
    }
}

#[test]
fn test_stop_netcode_server_unlinks_transport() {
    let mut stepper = ClientServerStepper::from_config(StepperConfig::single());
    let server = stepper.server_entity;

    stepper
        .server_app
        .world_mut()
        .trigger(Stop { entity: server });
    stepper.frame_step_server_first(10);

    let server = stepper.server();
    assert!(server.contains::<Stopped>());
    assert!(server.contains::<Unlinked>());
    assert!(!server.contains::<Linked>());
}

/// Check that the client/server setup is correct when the connection type is Raw instead of Netcode
#[test]
fn test_setup_raw_client_server() {
    let stepper = ClientServerStepper::from_config(StepperConfig::from_connection_types(
        vec![ClientType::Raw],
        ServerType::Raw,
    ));
    assert!(stepper.client(0).contains::<Connected>());

    assert!(stepper.client_of(0).contains::<Connected>());
}

#[test]
fn test_sender_metadata() {
    let stepper = ClientServerStepper::from_config(StepperConfig::single());
    let client = stepper.client(0).id();
    let client_of = stepper.client_of(0).id();

    assert_eq!(
        stepper
            .client_of(0)
            .get::<MessageManager>()
            .unwrap()
            .entity_mapper
            .get_local(client)
            .expect("client is not present in entity map"),
        client_of
    );
    // NOTE: the (client_of -> client) connection pair lives only in the local
    // `MessageManager` map (populated from `SenderMetadata`); replicon's map
    // only tracks replicated entities, so this lookup intentionally does not
    // go through `ServerEntityMap`.
    assert_eq!(
        stepper
            .client(0)
            .get::<MessageManager>()
            .unwrap()
            .entity_mapper
            .get_local(client_of)
            .expect("client_of is not present in entity map"),
        client
    );
}
