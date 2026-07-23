//! Handle input messages received from the clients

use crate::HISTORY_DEPTH;
#[cfg(feature = "prediction")]
use crate::InputChannel;
use crate::input_buffer::InputBuffer;
use crate::input_message::{
    ActionStateQueryData, ActionStateSequence, InputMessage, InputTarget, StateMut,
};
use crate::plugin::InputPlugin;
#[cfg(feature = "metrics")]
use alloc::format;
use bevy_app::{App, FixedPreUpdate, Plugin, PreUpdate};
use bevy_ecs::component::Component;
use bevy_ecs::prelude::Has;
use bevy_ecs::relationship::RelationshipTarget;
use bevy_ecs::{
    entity::{Entity, MapEntities},
    error::Result,
    query::With,
    resource::Resource,
    schedule::{IntoScheduleConfigs, SystemSet},
    system::{Commands, Query, Res, Single},
};
use bevy_utils::prelude::DebugName;
use core::fmt::{Debug, Formatter};
use core::time::Duration;
use lightyear_connection::client::Connected;
use lightyear_connection::host::HostServer;
use lightyear_connection::prelude::NetworkTarget;
use lightyear_connection::server::Started;
use lightyear_core::id::RemoteId;
use lightyear_core::prelude::LocalTimeline;
use lightyear_core::tick::{Tick, TickDuration};
use lightyear_link::prelude::{LinkOf, Server};
use lightyear_messages::plugin::MessageSystems;
use lightyear_messages::prelude::MessageReceiver;
use lightyear_messages::server::ServerMultiMessageSender;
use lightyear_replication::control::ControlledByRemote;
use lightyear_replication::prelude::{PreSpawned, RoomId, Rooms};
use tracing::{debug, error, trace};

/// Maximum number of ticks ahead of the server's current tick that an
/// incoming [`InputMessage::end_tick`] is allowed to be. Messages with
/// `end_tick > server_tick + MAX_INPUT_LOOKAHEAD_TICKS` are dropped before
/// they are written into the [`InputBuffer`].
///
/// Without this bound, [`InputBuffer::extend_to_range`] / [`InputBuffer::set_raw`]
/// extend the internal `VecDeque` to fit *any* tick value (filling intermediate
/// entries with `Absent` / `SameAsPrecedent`). A modified client sending
/// `end_tick = current + 30_000` would cause a 30 000-entry allocation per
/// message; repeated across messages and connections, the server is
/// memory-exhausted.
///
/// Legitimate clients run at most a few ticks ahead of the server (typical
/// `InputDelayConfig` values are 0–3 ticks). 64 ticks (~1 s at 64 Hz) is
/// generous compared to that range while still bounding attacker memory cost
/// per message.
const MAX_INPUT_LOOKAHEAD_TICKS: i32 = 64;

/// Maximum number of ticks *behind* the server's current tick that an incoming
/// [`InputMessage::end_tick`] is allowed to be.
///
/// Past-direction messages are normally handled harmlessly by
/// [`InputBuffer::set_raw`]'s start-tick guard. The explicit bound still prevents
/// arbitrarily old or malicious tick values from entering the input pipeline while accepting
/// reasonable late inputs (up to ~4 s of network lag at 64 Hz).
const MAX_INPUT_PAST_TICKS: i32 = 256;

/// Returns `true` iff `end_tick - server_tick` falls within
/// `[-MAX_INPUT_PAST_TICKS, MAX_INPUT_LOOKAHEAD_TICKS]`. See those constants
/// for the threat model behind each bound.
///
/// The subtraction is performed in `i64`, so every pair of ordinary `u32` ticks has an exact,
/// non-wrapping signed difference.
pub(crate) fn is_input_within_lookahead(end_tick: Tick, server_tick: Tick) -> bool {
    let delta = i64::from(end_tick.0) - i64::from(server_tick.0);
    (-i64::from(MAX_INPUT_PAST_TICKS)..=i64::from(MAX_INPUT_LOOKAHEAD_TICKS)).contains(&delta)
}

/// Server-side plugin that receives input messages from clients and applies
/// them to [`InputBuffer`] components.
///
/// If `rebroadcast_inputs` is enabled, the server also forwards input messages
/// to other clients so they can use them for remote-player prediction.
pub struct ServerInputPlugin<S> {
    pub rebroadcast_inputs: bool,
    pub marker: core::marker::PhantomData<S>,
}

impl<S> Default for ServerInputPlugin<S> {
    fn default() -> Self {
        Self {
            rebroadcast_inputs: false,
            marker: core::marker::PhantomData,
        }
    }
}

/// Runtime configuration for server-side input handling, inserted as a resource
/// by [`ServerInputPlugin`].
#[derive(Resource)]
pub struct ServerInputConfig<S> {
    pub rebroadcast_inputs: bool,
    pub marker: core::marker::PhantomData<S>,
}

#[deprecated(note = "Use InputSystems instead")]
pub type InputSet = InputSystems;

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum InputSystems {
    /// Validate / sanitize received [`InputMessage`]s before they are applied to
    /// the [`InputBuffer`]. Runs after `MessageSystems::Receive` and before
    /// [`Self::ReceiveInputs`]. Empty by default — add systems here (see
    /// [`InputValidationAppExt::add_input_validator`]) that mutate or drop
    /// messages via [`MessageReceiver::retain_messages`]. A game that wants to
    /// authorize input targets against `ControlledBy` can do so here.
    ValidateInputs,
    /// Receive the latest ActionDiffs from the client
    ReceiveInputs,
    /// Use the ActionDiff received from the client to update the `ActionState`
    UpdateActionState,
}

/// App-builder helper to register a server-side input-validation system.
///
/// The system runs in [`InputSystems::ValidateInputs`] — after messages are
/// received, before they are buffered — so it can mutate or drop them with full
/// ECS access (any `SystemParam`). It typically queries
/// `Query<&mut MessageReceiver<InputMessage<S>>>` and calls
/// [`MessageReceiver::retain_messages`]. This is sugar for
/// `app.add_systems(PreUpdate, system.in_set(InputSystems::ValidateInputs))`.
///
/// Validators in the set are unordered relative to each other. To make one run
/// before another, pass an ordered config — e.g.
/// `app.add_input_validator(my_validator.after(other_validator))`.
pub trait InputValidationAppExt {
    fn add_input_validator<M>(
        &mut self,
        systems: impl IntoScheduleConfigs<bevy_ecs::system::ScheduleSystem, M>,
    ) -> &mut Self;
}

impl InputValidationAppExt for App {
    fn add_input_validator<M>(
        &mut self,
        systems: impl IntoScheduleConfigs<bevy_ecs::system::ScheduleSystem, M>,
    ) -> &mut Self {
        self.add_systems(PreUpdate, systems.in_set(InputSystems::ValidateInputs));
        self
    }
}

/// Opt-in [`InputSystems::ValidateInputs`] system that strips every
/// `InputTarget::Entity` the sending peer is **not** authorized to control —
/// i.e. not a member of its [`ControlledByRemote`]. This is the spoofed-target
/// defense: a modified client cannot forge `InputTarget::Entity(other_player)`
/// to drive an entity it doesn't own. The message itself is kept (even if
/// filtering emptied it) — an empty input message is a legitimate keepalive the
/// receive path relies on; only the unauthorized targets are removed.
///
/// lightyear does **not** enable this by default. `ControlledBy` is an optional
/// helper for modeling input ownership, not a mandatory component, and some
/// games legitimately let several clients drive one entity. Register this only
/// if your game uses `ControlledBy` and wants the check:
///
/// ```ignore
/// app.add_input_validator(authorize_controlled_targets::<MySequence>);
/// ```
///
/// To run **your own** validation after this one — so it only sees authorized
/// targets — order it with `.after(authorize_controlled_targets::<S>)`
/// (validators in [`InputSystems::ValidateInputs`] are otherwise unordered):
///
/// ```ignore
/// app.add_input_validator(authorize_controlled_targets::<MySequence>);
/// app.add_input_validator(my_validator.after(authorize_controlled_targets::<MySequence>));
/// ```
///
/// - Host-client inputs (`RemoteId::is_local`) are trusted in-process and
///   skipped.
/// - `InputTarget::PreSpawned` is identified by a hash, not an entity id, so it
///   is passed through here (binding a prespawn to an owner is out of scope).
pub fn authorize_controlled_targets<S: ActionStateSequence>(
    mut receivers: Query<
        (
            &RemoteId,
            Option<&ControlledByRemote>,
            &mut MessageReceiver<InputMessage<S>>,
        ),
        With<Connected>,
    >,
) {
    for (client_id, controlled_by_remote, mut receiver) in receivers.iter_mut() {
        if client_id.is_local() {
            continue;
        }
        receiver.retain_messages(|message| {
            let before = message.inputs.len();
            message.inputs.retain(|data| match data.target {
                InputTarget::Entity(entity) => controlled_by_remote
                    .is_some_and(|controlled| controlled.collection().contains(&entity)),
                InputTarget::PreSpawned(_) => true,
            });
            let dropped = before - message.inputs.len();
            if dropped > 0 {
                trace!(
                    ?client_id,
                    dropped, "authorize_controlled_targets: stripped unauthorized input targets"
                );
            }
            // Keep the message even if filtering emptied it. An empty input
            // message is a legitimate keepalive (it still carries `end_tick`,
            // which the receive path needs — dropping it stalls the confirmed
            // tick and can trigger a large rollback). Only the unauthorized
            // *targets* are removed; the spoofed entries are already gone before
            // any rebroadcast.
            true
        });
    }
}

/// Component that is used to customize how inputs will be rebroadcasted
///
/// If absent, the inputs received on a given `ClientOf` entity will be rebroadcasted to all other clients
#[derive(Component)]
pub enum InputRebroadcaster<S> {
    // Rebroadcast to all users in the room
    Room(RoomId),
    Target(NetworkTarget),
    Marker(core::marker::PhantomData<S>),
}

impl<S> Debug for InputRebroadcaster<S> {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        match self {
            InputRebroadcaster::Room(id) => f.debug_tuple("Room").field(id).finish(),
            InputRebroadcaster::Target(target) => f.debug_tuple("Target").field(target).finish(),
            InputRebroadcaster::Marker(_) => f
                .debug_tuple("Marker")
                .field(&DebugName::type_name::<S>())
                .finish(),
        }
    }
}

impl<S> Default for InputRebroadcaster<S> {
    fn default() -> Self {
        Self::Target(NetworkTarget::All)
    }
}

impl<S: ActionStateSequence + MapEntities> Plugin for ServerInputPlugin<S> {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<InputPlugin<S>>() {
            app.add_plugins(InputPlugin::<S>::default());
        }
        app.insert_resource(ServerInputConfig::<S::Action> {
            rebroadcast_inputs: self.rebroadcast_inputs,
            marker: core::marker::PhantomData,
        });

        // SETS
        // TODO:
        //  - could there be an issue because, client updates `state` and `fixed_update_state` and sends it to server
        //  - server only considers `state` since we receive messages in PreUpdate
        //  - but host-server broadcasting their inputs only updates `state`
        app.configure_sets(
            PreUpdate,
            (
                MessageSystems::Receive,
                InputSystems::ValidateInputs,
                InputSystems::ReceiveInputs,
            )
                .chain(),
        );
        app.configure_sets(FixedPreUpdate, InputSystems::UpdateActionState);

        // for host server mode?
        #[cfg(feature = "client")]
        app.configure_sets(
            FixedPreUpdate,
            InputSystems::UpdateActionState.after(crate::client::InputSystems::BufferClientInputs),
        );

        // SYSTEMS
        app.add_systems(
            PreUpdate,
            receive_input_message::<S>.in_set(InputSystems::ReceiveInputs),
        );
        app.add_systems(
            FixedPreUpdate,
            update_action_state::<S>.in_set(InputSystems::UpdateActionState),
        );
    }
}

// TODO: why do we need the Server? we could just run this on any receiver.
//  (apart from rebroadcast inputs)

/// Read the input messages from the server events to update the InputBuffers
fn receive_input_message<S: ActionStateSequence>(
    config: Res<ServerInputConfig<S::Action>>,
    server: Query<&Server>,
    // make sure to only rebroadcast inputs to connected clients
    #[cfg_attr(not(feature = "prediction"), allow(unused_mut))]
    mut sender: ServerMultiMessageSender<With<Connected>>,
    tick_duration: Res<TickDuration>,
    rooms_query: Query<(Entity, &Rooms), With<Connected>>,
    timeline: Res<LocalTimeline>,
    mut receivers: Query<
        (
            Entity,
            &LinkOf,
            &mut MessageReceiver<InputMessage<S>>,
            &RemoteId,
            Option<&InputRebroadcaster<S::Action>>,
        ),
        // We also receive inputs from the HostClient, in case we want the HostClient's inputs to be
        // rebroadcast to other clients (so that they can do prediction of the HostClient's entity)
        With<Connected>,
    >,
    mut query: Query<Option<&mut InputBuffer<S::Snapshot, S::Action>>>,
    prespawned: Query<
        (Entity, &PreSpawned),
        (
            With<<S::State as ActionStateQueryData>::Main>,
            With<InputBuffer<S::Snapshot, S::Action>>,
        ),
    >,
    mut commands: Commands,
) -> Result {
    // TODO: use par_iter_mut
    receivers.iter_mut().try_for_each(|(client_entity, link_of, mut receiver, client_id, rebroadcaster)| {
        // TODO: this drains the messages... but the user might want to re-broadcast them?
        //  should we just read instead?
        let server_entity = link_of.server;
        let tick = timeline.tick();
        receiver.receive().try_for_each(|message| {
            #[cfg(feature = "prediction")]
            let mut message = message;
            // ignore input messages from the local client (if running in host-server mode)
            // if we're not doing rebroadcasting
            if client_id.is_local() && !config.rebroadcast_inputs {
                error!("Received input message from HostClient for action {:?} even though rebroadcasting is disabled. Ignoring the message.", DebugName::type_name::<S::Action>().shortname());
                return Ok(())
            }
            // NOTE: This can cause issues because the clients expect a steady stream of messages.
            //  For example the LastConfirmedTick could be really old which would cause a massive rollback
            // if message.is_empty() {
            //     return Ok(())
            // }
            trace!(?tick, ?client_id, action = ?DebugName::type_name::<S::Action>().shortname(), ?message.end_tick, ?message.inputs, "received input message");
            trace!(
                target: "lightyear_debug::input",
                kind = "server_input_message_recv",
                schedule = "PreUpdate",
                sample_point = "PreUpdate",
                entity = ?client_entity,
                server_entity = ?server_entity,
                client_id = ?client_id.0,
                action = ?DebugName::type_name::<S::Action>(),
                local_tick = tick.0,
                end_tick = message.end_tick.0,
                num_targets = message.inputs.len(),
                rebroadcast = message.rebroadcast,
                message = ?message,
                "server received input message"
            );

            // Reject messages whose end_tick is implausibly far from the
            // server's current tick before any buffer write. A modified
            // client sending a far-future end_tick would otherwise force
            // `InputBuffer::set_raw` to allocate one entry per intermediate
            // tick (memory-exhaustion DoS). See `is_input_within_lookahead`.
            if !is_input_within_lookahead(message.end_tick, tick) {
                trace!(
                    ?tick,
                    ?client_id,
                    end_tick = ?message.end_tick,
                    "Dropping input message: end_tick outside [server-{}, server+{}] window",
                    MAX_INPUT_PAST_TICKS,
                    MAX_INPUT_LOOKAHEAD_TICKS,
                );
                return Ok(())
            }

            // TODO: or should we try to store in a buffer the interpolation delay for the exact tick
            //  that the message was intended for?
            #[cfg(feature = "interpolation")]
            if let Some(interpolation_delay) = message.interpolation_delay {
                // update the interpolation delay estimate for the client
                commands.entity(client_entity).insert(interpolation_delay);
            }

            #[cfg(feature = "prediction")]
            if config.rebroadcast_inputs && let Ok(server) = server.get(server_entity) {
                // only rebroadcast if the message is not already a rebroadcast
                if !message.rebroadcast {
                    // Resolve PreSpawned targets to server entities before rebroadcasting,
                    // so that other clients can resolve them via normal entity mapping.
                    for input in message.inputs.iter_mut() {
                        if let InputTarget::PreSpawned(hash) = input.target
                            && let Some(server_e) = prespawned.iter()
                                .find_map(|(e, p)| p.hash.is_some_and(|h| h == hash).then_some(e))
                        {
                            input.target = InputTarget::Entity(server_e);
                        }
                    }
                    debug!(action = ?DebugName::type_name::<S>().shortname(), "Rebroadcast input message {message:?} from client {client_id:?} with rebroadcaster {rebroadcaster:?}");
                    message.rebroadcast = true;
                    trace!(
                        target: "lightyear_debug::input",
                        kind = "server_input_rebroadcast",
                        schedule = "PreUpdate",
                        sample_point = "PreUpdate",
                        entity = ?client_entity,
                        server_entity = ?server_entity,
                        client_id = ?client_id.0,
                        action = ?DebugName::type_name::<S::Action>(),
                        local_tick = tick.0,
                        end_tick = message.end_tick.0,
                        rebroadcaster = ?rebroadcaster,
                        num_targets = message.inputs.len(),
                        "server rebroadcasting input message"
                    );
                    match rebroadcaster {
                        None => {
                            sender.send::<_, InputChannel>(
                                &message,
                                server,
                                &NetworkTarget::AllExceptSingle(client_id.0)
                            )?;
                        }
                        Some(InputRebroadcaster::Room(room)) => {
                            let targets: bevy_ecs::entity::EntityHashSet = rooms_query.iter()
                                .filter(|(e, rooms)| *e != client_entity && rooms.contains_room(*room))
                                .map(|(e, _)| e)
                                .collect();
                            sender.send_to_entities::<_, InputChannel>(
                                &message,
                                &targets
                            )?;
                        },
                        Some(InputRebroadcaster::Target(target)) => {
                            sender.send::<_, InputChannel>(
                                &message,
                                server,
                                target
                            )?;
                        }
                        Some(InputRebroadcaster::Marker(_)) => unreachable!()
                    }
                }
            }

            for data in message.inputs {
                let Some(entity) = (match data.target {
                    InputTarget::Entity(entity) => {
                        Some(entity)
                    },
                    InputTarget::PreSpawned(hash) => {
                        debug!(?hash, "Received input for prespawned entity");
                        // We cannot match using the PreSpawnedReceiver since it only stores hashes for entities
                        // with no Replicate component, so resolve the input target against server-side input entities.
                        prespawned
                            .iter()
                            .filter_map(|(e, p)| p.hash.is_some_and(|h| h == hash).then_some(e)).next()
                    }
                }) else {
                    debug!(?data.states, ?data.target, end_tick = ?message.end_tick, "received input message for unrecognized entity");
                    continue
                };
                if let Ok(buffer) = query.get_mut(entity) {
                    if let Some(mut buffer) = buffer {
                        trace!(
                            "Updating InputBuffer: {} using: {:?}",
                            buffer.as_ref(),
                            data.states
                        );
                        let previous_last_remote_tick = buffer.last_remote_tick;
                        if let Some((rewrite_tick, previous, incoming)) =
                            detect_input_history_rewrite::<S>(
                                data.states.clone(),
                                &buffer,
                                message.end_tick,
                                tick_duration.0,
                            )
                        {
                            if rewrite_tick < tick {
                                error!(
                                    target: "lightyear_debug::input",
                                    kind = "server_input_history_rewrite",
                                    schedule = "PreUpdate",
                                    sample_point = "PreUpdate",
                                    entity = ?entity,
                                    client_entity = ?client_entity,
                                    client_id = ?client_id.0,
                                    action = ?DebugName::type_name::<S::Action>(),
                                    local_tick = tick.0,
                                    rewrite_tick = rewrite_tick.0,
                                    end_tick = message.end_tick.0,
                                    previous_last_remote_tick = ?previous_last_remote_tick,
                                    already_simulated = true,
                                    previous = ?previous,
                                    incoming = ?incoming,
                                    buffer_len = buffer.len(),
                                    input_buffer = %*buffer,
                                    "server received a different input for an already-simulated tick"
                                );
                            } else {
                                trace!(
                                    target: "lightyear_debug::input",
                                    kind = "server_input_history_rewrite",
                                    schedule = "PreUpdate",
                                    sample_point = "PreUpdate",
                                    entity = ?entity,
                                    client_entity = ?client_entity,
                                    client_id = ?client_id.0,
                                    action = ?DebugName::type_name::<S::Action>(),
                                    local_tick = tick.0,
                                    rewrite_tick = rewrite_tick.0,
                                    end_tick = message.end_tick.0,
                                    previous_last_remote_tick = ?previous_last_remote_tick,
                                    already_simulated = false,
                                    previous = ?previous,
                                    incoming = ?incoming,
                                    buffer_len = buffer.len(),
                                    input_buffer = %*buffer,
                                    "server received a different input for a future tick already covered by an earlier client input packet"
                                );
                            }
                        }
                        let mismatch =
                            data.states
                                .update_buffer(&mut buffer, message.end_tick, tick_duration.0);
                        if let Some(mismatch_tick) = mismatch
                            && mismatch_tick < tick
                        {
                            error!(
                                target: "lightyear_debug::input",
                                kind = "server_late_input_mismatch",
                                schedule = "PreUpdate",
                                sample_point = "PreUpdate",
                                entity = ?entity,
                                client_entity = ?client_entity,
                                client_id = ?client_id.0,
                                action = ?DebugName::type_name::<S::Action>(),
                                local_tick = tick.0,
                                mismatch_tick = mismatch_tick.0,
                                end_tick = message.end_tick.0,
                                previous_last_remote_tick = ?previous_last_remote_tick,
                                last_remote_tick = ?buffer.last_remote_tick,
                                buffer_len = buffer.len(),
                                input_buffer = %*buffer,
                                "server received an input correction for an already-simulated tick"
                            );
                        }
                        trace!(
                            target: "lightyear_debug::input",
                            kind = "server_input_buffer_update",
                            schedule = "PreUpdate",
                            sample_point = "PreUpdate",
                            entity = ?entity,
                            client_id = ?client_id.0,
                            action = ?DebugName::type_name::<S::Action>(),
                            local_tick = tick.0,
                            end_tick = message.end_tick.0,
                            buffer_len = buffer.len(),
                            input_buffer = %*buffer,
                            "server updated input buffer"
                        );
                    } else {
                        debug!("Adding InputBuffer and ActionState which are missing on the entity");
                        let mut buffer = InputBuffer::<S::Snapshot, S::Action>::default();
                        let mismatch =
                            data.states
                                .update_buffer(&mut buffer, message.end_tick, tick_duration.0);
                        if let Some(mismatch_tick) = mismatch
                            && mismatch_tick < tick
                        {
                            error!(
                                target: "lightyear_debug::input",
                                kind = "server_late_input_mismatch",
                                schedule = "PreUpdate",
                                sample_point = "PreUpdate",
                                entity = ?entity,
                                client_entity = ?client_entity,
                                client_id = ?client_id.0,
                                action = ?DebugName::type_name::<S::Action>(),
                                local_tick = tick.0,
                                mismatch_tick = mismatch_tick.0,
                                end_tick = message.end_tick.0,
                                previous_last_remote_tick = ?None::<lightyear_core::tick::Tick>,
                                last_remote_tick = ?buffer.last_remote_tick,
                                buffer_len = buffer.len(),
                                input_buffer = %buffer,
                                "server received initial input for an already-simulated tick"
                            );
                        }
                        trace!(
                            target: "lightyear_debug::input",
                            kind = "server_input_buffer_insert",
                            schedule = "PreUpdate",
                            sample_point = "PreUpdate",
                            entity = ?entity,
                            client_id = ?client_id.0,
                            action = ?DebugName::type_name::<S::Action>(),
                            local_tick = tick.0,
                            end_tick = message.end_tick.0,
                            buffer_len = buffer.len(),
                            input_buffer = %buffer,
                            "server inserted input buffer"
                        );
                        commands.entity(entity).insert((
                            buffer,
                            S::State::base_value()
                        ));
                        // commands.command_scope(|mut commands| {
                        //     commands.entity(entity).insert((
                        //         buffer,
                        //         ActionState::<A>::default(),
                        //     ));
                        // });
                    }
                } else {
                    debug!(?entity, ?data.states, end_tick = ?message.end_tick, "received input message for non-existing entity");
                }
            }
            Ok(())
        })
    })
}

fn detect_input_history_rewrite<S: ActionStateSequence>(
    states: S,
    input_buffer: &InputBuffer<S::Snapshot, S::Action>,
    end_tick: lightyear_core::tick::Tick,
    tick_duration: Duration,
) -> Option<(
    lightyear_core::tick::Tick,
    Option<S::Snapshot>,
    Option<S::Snapshot>,
)> {
    let last_remote_tick = input_buffer.last_remote_tick?;
    let buffer_start_tick = input_buffer.start_tick?;
    let buffer_end_tick = input_buffer.end_tick()?;
    let start_tick = end_tick + 1 - states.len() as u32;
    let mut incoming = None;
    for (delta, input) in states.get_snapshots_from_message(tick_duration).enumerate() {
        let tick = start_tick + lightyear_core::tick::Tick(delta as u32);
        match input {
            crate::input_buffer::Compressed::Absent => incoming = None,
            crate::input_buffer::Compressed::Input(value) => incoming = Some(value),
            crate::input_buffer::Compressed::SameAsPrecedent => {}
        }
        if tick <= last_remote_tick {
            // The server keeps very little input history after simulating a
            // tick, so ordinary redundant input packets can mention older
            // ticks that have already been popped from the buffer. Those
            // cannot be compared reliably here.
            if tick < buffer_start_tick || tick > buffer_end_tick {
                continue;
            }
            let previous = input_buffer.get(tick).cloned();
            if previous != incoming {
                return Some((tick, previous, incoming));
            }
        }
    }
    None
}

/// Read the InputState for the current tick from the buffer, and use them to update the ActionState
///
/// NOTE: this will also run on HostClients! This is why we disable `get_action_state` in the client
/// plugin for host-clients. This system also removes old inputs from the buffer, which is why we
/// can also skip `clear_buffers` on host-clients
fn update_action_state<S: ActionStateSequence>(
    // TODO: what if there are multiple servers? maybe we can use Replicate to figure out which inputs should be replicating on which servers?
    //  and use the timeline from that connection? i.e. find from which entity we got the first InputMessage?
    //  presumably the entity is replicated to many clients, but only one client is controlling the entity?
    timeline: Res<LocalTimeline>,
    server: Single<(Entity, Has<HostServer>), With<Started>>,
    mut action_state_query: Query<(
        Entity,
        StateMut<S>,
        &mut InputBuffer<S::Snapshot, S::Action>,
    )>,
) {
    let (server, host_client) = server.into_inner();
    let tick = timeline.tick();
    for (entity, action_state, mut input_buffer) in action_state_query.iter_mut() {
        trace!(?tick, ?server, ?input_buffer, "input buffer on server");
        // We only apply the ActionState from the buffer if we have one.
        // If we don't (because the input packet is late or lost), we won't do anything.
        // This is equivalent to considering that the player will keep playing the last action they played.
        if let Some(snapshot) = input_buffer.get_predict(tick) {
            S::from_snapshot_transitions(S::State::into_inner(action_state), snapshot);
            trace!(
                ?tick,
                ?entity,
                "action state after update. Input Buffer: {}",
                input_buffer.as_ref()
            );
            trace!(
                target: "lightyear_debug::input",
                kind = "server_update_action_state",
                schedule = "FixedPreUpdate",
                sample_point = "FixedPreUpdate",
                entity = ?entity,
                server_entity = ?server,
                action = ?DebugName::type_name::<S::Action>(),
                local_tick = tick.0,
                input_tick = tick.0,
                host_client,
                snapshot = ?snapshot,
                buffer_len = input_buffer.len(),
                input_buffer = %input_buffer.as_ref(),
                "server applied input buffer to action state"
            );

            #[cfg(feature = "metrics")]
            {
                // The size of the buffer should always bet at least 1, and hopefully be a bit more than that
                // so that we can handle lost messages
                metrics::gauge!(format!(
                    "inputs::{}::{}::buffer_size",
                    DebugName::type_name::<S::Action>(),
                    entity
                ))
                .set(input_buffer.len() as f64);
            }
        }

        // NOTE: if we are the host-client, it is important to keep some history in the inputs
        // The reason is that we are sending our inputs to other clients, which might cause rollbacks.
        // For example there are new inputs starting from tick 7: L, L, L
        // But the other clients might receive the message from tick 9 first (because of reordering), in which case it
        // is important that they know that the action L was first pressed at tick 7! If the history is cut too short,
        // then that information is not included in the message
        // Basically, in host-client we are producer of inputs, so we need to include some redundancy. (like when
        // normal clients send inputs)
        let history_depth = if host_client {
            HISTORY_DEPTH
        } else {
            // if we are a server and not a host-client, there is no need to keep history
            1
        };
        // TODO: + we also want to keep enough inputs on the client to be able to do prediction effectively!
        // remove all the previous values
        // we keep the current value in the InputBuffer so that if future messages are lost, we can still
        // fallback on the last known value
        input_buffer.pop_keeping_last(tick - history_depth);
        // info!("Buffer length: {}", input_buffer.len());
    }
}

#[cfg(test)]
mod lookahead_tests {
    use super::*;

    /// Forward bound is inclusive.
    #[test]
    fn accepts_within_forward_bound() {
        let server = Tick(1_000);
        assert!(is_input_within_lookahead(server, server));
        assert!(is_input_within_lookahead(
            server + MAX_INPUT_LOOKAHEAD_TICKS,
            server
        ));
    }

    /// One tick past the forward bound is rejected.
    #[test]
    fn rejects_beyond_forward_bound() {
        let server = Tick(1_000);
        assert!(!is_input_within_lookahead(
            server + (MAX_INPUT_LOOKAHEAD_TICKS + 1),
            server
        ));
    }

    /// Reasonable late inputs (within the past bound) are accepted.
    #[test]
    fn accepts_within_past_bound() {
        let server = Tick(1_000);
        assert!(is_input_within_lookahead(
            server + (-MAX_INPUT_PAST_TICKS),
            server
        ));
    }

    /// Far-future end_tick (the DoS payload) is rejected.
    #[test]
    fn rejects_far_future_end_tick() {
        let server = Tick(1_000);
        assert!(!is_input_within_lookahead(server + 30_000, server));
    }

    /// The largest possible ordinary tick is still classified as far future, never as past.
    #[test]
    fn rejects_max_tick_as_far_future() {
        let server = Tick(1_000);
        assert!(!is_input_within_lookahead(Tick(u32::MAX), server));
    }
}
