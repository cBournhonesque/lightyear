//! Link-level metadata shared by debug sampling systems.

use alloc::string::{String, ToString};
use core::sync::atomic::{AtomicU64, Ordering};

use bevy_app::{App, First, Plugin, PreUpdate};
use bevy_ecs::prelude::*;
use bevy_reflect::Reflect;
use lightyear_connection::client::Client;
use lightyear_connection::host::HostClient;
use lightyear_core::id::{LocalId, PeerId, RemoteId};
use lightyear_link::Link;
use lightyear_link::prelude::Server;

use crate::debug::component::LightyearDebugComponentSamplerPlugin;

static DEBUG_FRAME_ID: AtomicU64 = AtomicU64::new(0);

/// Return the frame id most recently published by [`LightyearDebugPlugin`].
#[inline]
pub fn current_debug_frame_id() -> u64 {
    DEBUG_FRAME_ID.load(Ordering::Relaxed)
}

/// Publish the frame id used by the structured tracing layer.
#[inline]
pub fn set_current_debug_frame_id(frame_id: u64) {
    DEBUG_FRAME_ID.store(frame_id, Ordering::Relaxed);
}

/// Role of the app/link that emitted a debug row.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Reflect)]
pub enum LightyearDebugRole {
    Client,
    Server,
    HostClient,
    #[default]
    Unknown,
}

impl LightyearDebugRole {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Client => "client",
            Self::Server => "server",
            Self::HostClient => "host_client",
            Self::Unknown => "unknown",
        }
    }
}

/// Common debug metadata attached to each [`Link`] entity.
///
/// Debug samplers can read this component and include its fields in their
/// `lightyear_debug::*` tracing events.
#[derive(Component, Debug, Default, Clone, PartialEq, Eq, Reflect)]
pub struct LightyearDebugMetadata {
    pub role: LightyearDebugRole,
    pub client_id: Option<u64>,
    pub local_id: Option<String>,
    pub remote_id: Option<String>,
}

/// Per-app frame counter used to annotate debug rows.
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Eq, Reflect)]
pub struct LightyearDebugFrame {
    pub id: u64,
}

/// Maintains [`LightyearDebugMetadata`] on link entities.
#[derive(Default)]
pub struct LightyearDebugPlugin;

impl Plugin for LightyearDebugPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<LightyearDebugRole>()
            .register_type::<LightyearDebugMetadata>()
            .register_type::<LightyearDebugFrame>()
            .init_resource::<LightyearDebugFrame>()
            .add_plugins(LightyearDebugComponentSamplerPlugin)
            .add_systems(First, advance_debug_frame)
            .add_systems(PreUpdate, update_link_debug_metadata);
    }
}

fn advance_debug_frame(mut frame: ResMut<LightyearDebugFrame>) {
    frame.id = frame.id.saturating_add(1);
    set_current_debug_frame_id(frame.id);
}

fn update_link_debug_metadata(
    mut commands: Commands,
    mut links: Query<
        (
            Entity,
            Option<&LocalId>,
            Option<&RemoteId>,
            Has<Client>,
            Has<Server>,
            Has<HostClient>,
            Option<&mut LightyearDebugMetadata>,
        ),
        With<Link>,
    >,
) {
    for (entity, local_id, remote_id, has_client, has_server, has_host_client, metadata) in
        &mut links
    {
        let next = LightyearDebugMetadata {
            role: role(has_client, has_server, has_host_client),
            client_id: client_id(
                local_id.map(|id| id.0),
                remote_id.map(|id| id.0),
                has_server,
            ),
            local_id: local_id.map(ToString::to_string),
            remote_id: remote_id.map(ToString::to_string),
        };

        if let Some(mut metadata) = metadata {
            if *metadata != next {
                *metadata = next;
            }
        } else {
            commands.entity(entity).insert(next);
        }
    }
}

fn role(has_client: bool, has_server: bool, has_host_client: bool) -> LightyearDebugRole {
    if has_host_client {
        LightyearDebugRole::HostClient
    } else if has_server {
        LightyearDebugRole::Server
    } else if has_client {
        LightyearDebugRole::Client
    } else {
        LightyearDebugRole::Unknown
    }
}

fn client_id(local_id: Option<PeerId>, remote_id: Option<PeerId>, is_server: bool) -> Option<u64> {
    if is_server {
        remote_id.and_then(peer_numeric_id)
    } else {
        local_id.and_then(peer_numeric_id)
    }
    .or_else(|| local_id.and_then(peer_numeric_id))
    .or_else(|| remote_id.and_then(peer_numeric_id))
}

fn peer_numeric_id(peer_id: PeerId) -> Option<u64> {
    match peer_id {
        PeerId::Entity(id) | PeerId::Netcode(id) | PeerId::Steam(id) | PeerId::Local(id) => {
            Some(id)
        }
        PeerId::Raw(_) | PeerId::Server => None,
    }
}
