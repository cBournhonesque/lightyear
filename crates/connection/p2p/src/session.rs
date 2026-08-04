use bevy_app::{App, Plugin, PreUpdate};
use bevy_ecs::prelude::*;
use bevy_time::{Time, Virtual};
use lightyear_connection::client::Connected;
use lightyear_connection::direction::NetworkDirection;
use lightyear_connection::network_topology::{
    NetworkTopology, NetworkTopologyError, NetworkTopologySystems, NetworkingMetadata,
};
use lightyear_connection::p2p::{AuthenticatedPeerId, P2P};
use lightyear_core::id::{LocalId, PeerId, RemoteId};
use lightyear_core::prelude::{LocalTimeline, Tick};
use lightyear_messages::plugin::MessageSystems;
use lightyear_messages::prelude::{AppMessageExt, MessageReceiver, MessageSender};
use lightyear_sync::prelude::{P2PTimelineDiverged, SyncedInputTimeline};
use lightyear_transport::prelude::{AppChannelExt, ChannelMode, ChannelSettings, ReliableSettings};
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

const MAX_ROSTER_SIZE: usize = 4;

/// Identifies one match or lobby instance.
///
/// A newly established Link cannot join a session whose identifier differs, even if its roster
/// and gameplay configuration happen to match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct P2PSessionId(pub [u8; 16]);

impl P2PSessionId {
    /// Construct a compact session identifier for tests or application-defined lobby IDs.
    pub fn from_u128(value: u128) -> Self {
        Self(value.to_le_bytes())
    }
}

/// Fingerprint of application-owned deterministic configuration.
///
/// Lightyear compares this value across the roster but deliberately does not prescribe how an
/// application hashes its protocol, map, seed, input delay, or gameplay settings. Production
/// applications should use a collision-resistant hash of a canonical encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct P2PConfigFingerprint(pub [u8; 32]);

impl P2PConfigFingerprint {
    /// Construct a fingerprint from a small application identifier.
    ///
    /// This is convenient for examples and tests, but is not collision resistant.
    pub fn from_u64(value: u64) -> Self {
        let mut bytes = [0; 32];
        bytes[..8].copy_from_slice(&value.to_le_bytes());
        Self(bytes)
    }
}

/// What the session does when a roster member is lost after handshaking begins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum P2PPeerLossPolicy {
    /// Stop virtual time and transition to a terminal aborted state.
    Abort,
    /// Stop virtual time and remain paused for application-owned recovery or lobby UI.
    ///
    /// Automatic reconnection is intentionally absent until the input protocol can guarantee
    /// recovery of every historical input missing across the interruption.
    Pause,
}

/// Whether configured Link identities must be certified by their connection backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum P2PIdentityPolicy {
    /// Require [`AuthenticatedPeerId`] on every P2P Link.
    RequireAuthenticated,
    /// Trust each Link's configured [`RemoteId`] without authentication.
    ///
    /// This exists for local raw-transport examples and tests. It must not be used to admit
    /// untrusted Internet peers.
    TrustLinkIdentity,
}

/// Configuration that every peer must agree on before proposing a start tick.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct P2PAgreement {
    /// Match/lobby identity.
    pub session_id: P2PSessionId,
    /// Canonically ordered complete roster, including the local peer.
    ///
    /// The position in this list is also a stable player slot, so peers must agree on both its
    /// contents and ordering.
    pub roster: SmallVec<[PeerId; MAX_ROSTER_SIZE]>,
    /// Application-owned deterministic configuration fingerprint.
    pub configuration: P2PConfigFingerprint,
    /// Number of ticks added after the latest ready proposal to choose the shared future start.
    pub start_delay_ticks: u16,
    /// Agreed behavior if a roster peer disappears after handshaking starts.
    pub peer_loss_policy: P2PPeerLossPolicy,
}

/// Local construction settings for a [`P2PSession`].
#[derive(Debug, Clone)]
pub struct P2PSessionConfig {
    local_peer_id: PeerId,
    agreement: P2PAgreement,
    identity_policy: P2PIdentityPolicy,
}

impl P2PSessionConfig {
    /// Build and validate one fixed-roster session configuration.
    pub fn new(
        session_id: P2PSessionId,
        local_peer_id: PeerId,
        roster: impl IntoIterator<Item = PeerId>,
        configuration: P2PConfigFingerprint,
    ) -> Result<Self, P2PSessionConfigError> {
        let roster: SmallVec<[PeerId; MAX_ROSTER_SIZE]> = roster.into_iter().collect();
        if !(2..=MAX_ROSTER_SIZE).contains(&roster.len()) {
            return Err(P2PSessionConfigError::RosterSize(roster.len()));
        }
        for (index, peer) in roster.iter().enumerate() {
            if roster[..index].contains(peer) {
                return Err(P2PSessionConfigError::DuplicatePeer(*peer));
            }
        }
        if !roster.contains(&local_peer_id) {
            return Err(P2PSessionConfigError::LocalPeerMissing(local_peer_id));
        }

        Ok(Self {
            local_peer_id,
            agreement: P2PAgreement {
                session_id,
                roster,
                configuration,
                start_delay_ticks: 120,
                peer_loss_policy: P2PPeerLossPolicy::Abort,
            },
            identity_policy: P2PIdentityPolicy::RequireAuthenticated,
        })
    }

    /// Set how far in the future the all-to-all start proposal is scheduled.
    pub fn with_start_delay_ticks(mut self, ticks: u16) -> Self {
        self.agreement.start_delay_ticks = ticks;
        self
    }

    /// Set the behavior used after losing a peer.
    pub fn with_peer_loss_policy(mut self, policy: P2PPeerLossPolicy) -> Self {
        self.agreement.peer_loss_policy = policy;
        self
    }

    /// Set the local identity verification requirement.
    pub fn with_identity_policy(mut self, policy: P2PIdentityPolicy) -> Self {
        self.identity_policy = policy;
        self
    }

    /// Local peer identity expected on every direct Link.
    pub fn local_peer_id(&self) -> PeerId {
        self.local_peer_id
    }

    /// Configuration exchanged with every roster member.
    pub fn agreement(&self) -> &P2PAgreement {
        &self.agreement
    }

    /// Identity verification policy applied locally to connection Links.
    pub fn identity_policy(&self) -> P2PIdentityPolicy {
        self.identity_policy
    }
}

/// Invalid local fixed-roster configuration.
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum P2PSessionConfigError {
    /// The first implementation supports two through four roster members.
    #[error("P2P roster size must be between 2 and 4, got {0}")]
    RosterSize(usize),
    /// A peer identity appeared more than once in the roster.
    #[error("P2P roster contains duplicate peer {0}")]
    DuplicatePeer(PeerId),
    /// The configured local identity was absent from the complete roster.
    #[error("local peer {0} is missing from the P2P roster")]
    LocalPeerMissing(PeerId),
}

/// Field that differed in another peer's session agreement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum P2PAgreementMismatch {
    /// Match/lobby identity differs.
    SessionId,
    /// Roster contents or stable ordering differs.
    Roster,
    /// Application configuration fingerprint differs.
    Configuration,
    /// Start lead differs.
    StartDelay,
    /// Peer-loss policy differs.
    PeerLossPolicy,
}

/// Fatal session validation or protocol failure.
#[derive(thiserror::Error, Debug, Clone, PartialEq)]
pub enum P2PSessionError {
    /// Ready network entities do not form a P2P topology.
    #[error("P2P session observed a non-P2P topology")]
    WrongTopology,
    /// The cached networking topology is invalid.
    #[error("invalid networking topology: {0}")]
    InvalidTopology(NetworkTopologyError),
    /// More P2P Links were declared than the fixed roster permits.
    #[error("declared {declared} P2P Links for a roster that expects {expected}")]
    DeclaredLinkCount { declared: u8, expected: u8 },
    /// A connected topology Link disappeared before it could be inspected.
    #[error("connected P2P Link {link:?} is no longer available")]
    MissingLink { link: Entity },
    /// A connected P2P Link has no local identity.
    #[error("P2P Link {link:?} has no LocalId")]
    MissingLocalId { link: Entity },
    /// A connected P2P Link has no remote identity.
    #[error("P2P Link {link:?} has no RemoteId")]
    MissingRemoteId { link: Entity },
    /// A Link's local identity differs from the session configuration.
    #[error("P2P Link {link:?} uses local peer {actual}, expected {expected}")]
    LocalIdentityMismatch {
        /// Link entity.
        link: Entity,
        /// Configured identity.
        expected: PeerId,
        /// Link identity.
        actual: PeerId,
    },
    /// A remote identity is not in the agreed roster.
    #[error("P2P Link {link:?} identifies unexpected peer {peer}")]
    UnexpectedPeer { link: Entity, peer: PeerId },
    /// A session message was attributed to an identity outside the agreed roster.
    #[error("received a P2P session message from unexpected peer {peer}")]
    MessageFromUnexpectedPeer { peer: PeerId },
    /// Two connected Links identify the same roster member.
    #[error("multiple P2P Links identify peer {peer}: {first:?} and {second:?}")]
    DuplicatePeerLink {
        /// Duplicated peer.
        peer: PeerId,
        /// First Link.
        first: Entity,
        /// Second Link.
        second: Entity,
    },
    /// The configured connection backend did not certify the Link identity.
    #[error("P2P Link {link:?} for peer {peer} is not authenticated")]
    UnauthenticatedPeer { link: Entity, peer: PeerId },
    /// The P2P Link is missing the typed session-message sender installed by the plugin.
    #[error("P2P Link {link:?} has no session message sender")]
    MissingMessageSender { link: Entity },
    /// A peer advertised a different fixed-session agreement.
    #[error("peer {peer} disagrees on P2P session field {field:?}")]
    AgreementMismatch {
        /// Remote peer.
        peer: PeerId,
        /// First field that differs.
        field: P2PAgreementMismatch,
    },
    /// A non-Hello message carried another session identifier.
    #[error("peer {peer} sent a message for another P2P session")]
    WrongSessionMessage { peer: PeerId },
    /// A peer sent start negotiation data before its Hello was accepted.
    #[error("peer {peer} sent start negotiation data before agreeing on the session")]
    StartBeforeHello { peer: PeerId },
    /// One peer changed its one-shot ready proposal.
    #[error("peer {peer} changed its start proposal from {first:?} to {second:?}")]
    ConflictingStartProposal {
        /// Remote peer.
        peer: PeerId,
        /// First proposal.
        first: Tick,
        /// Conflicting proposal.
        second: Tick,
    },
    /// One peer changed its one-shot start acknowledgement.
    #[error("peer {peer} changed its start acknowledgement from {first:?} to {second:?}")]
    ConflictingStartAcknowledgement {
        /// Remote peer.
        peer: PeerId,
        /// First acknowledgement.
        first: Tick,
        /// Conflicting acknowledgement.
        second: Tick,
    },
    /// One peer acknowledged a start tick different from the locally derived tick.
    #[error("peer {peer} acknowledged start tick {actual:?}, expected {expected:?}")]
    StartAcknowledgementMismatch {
        /// Remote peer.
        peer: PeerId,
        /// Derived start tick.
        expected: Tick,
        /// Remote acknowledgement.
        actual: Tick,
    },
    /// Agreement completed only after its proposed start tick had arrived.
    #[error("agreed P2P start tick {start_tick:?} elapsed at local tick {current_tick:?}")]
    StartEpochElapsed {
        /// Agreed start tick.
        start_tick: Tick,
        /// Current local tick.
        current_tick: Tick,
    },
    /// A roster member disappeared after handshaking began.
    #[error("lost P2P roster peer {peer}")]
    PeerLost { peer: PeerId },
    /// Existing P2P phase synchronization exceeded its bounded correction range.
    #[error("P2P timeline diverged on Link {limiting_link:?} by {lead} ticks")]
    TimelineDiverged {
        /// Link that produced the limiting observation.
        limiting_link: Entity,
        /// Fractional-tick lead.
        lead: f32,
    },
}

/// Public lifecycle of one fixed-roster session.
#[derive(Debug, Clone, PartialEq)]
pub enum P2PSessionState {
    /// Waiting for the declared Link set to match and connect to the configured roster.
    WaitingForPeers,
    /// Exchanging and validating roster/configuration Hellos.
    Handshaking,
    /// Agreement is complete, but the existing input timeline has not synchronized yet.
    WaitingForTimeline,
    /// Exchanging one-shot ready proposals and acknowledgements.
    AgreeingStart,
    /// Every peer acknowledged the same future simulation tick.
    Starting { start_tick: Tick },
    /// Gameplay may run for the fixed roster.
    Running { start_tick: Tick },
    /// A lost peer paused virtual time for application-owned recovery.
    Paused { peer: PeerId },
    /// The session ended and virtual time was paused.
    Aborted { error: P2PSessionError },
}

#[derive(Debug, Clone)]
struct PeerState {
    peer_id: PeerId,
    link: Option<Entity>,
    hello_sent: bool,
    hello_received: bool,
    proposal_sent: bool,
    proposal: Option<Tick>,
    acknowledgement_sent: Option<Tick>,
    acknowledgement: Option<Tick>,
}

impl PeerState {
    fn new(peer_id: PeerId, is_local: bool) -> Self {
        Self {
            peer_id,
            link: None,
            hello_sent: is_local,
            hello_received: is_local,
            proposal_sent: is_local,
            proposal: None,
            acknowledgement_sent: None,
            acknowledgement: None,
        }
    }
}

/// Application-global fixed-roster P2P session state.
#[derive(Resource, Debug, Clone)]
pub struct P2PSession {
    config: P2PSessionConfig,
    state: P2PSessionState,
    peers: SmallVec<[PeerState; MAX_ROSTER_SIZE]>,
}

impl P2PSession {
    /// Create a session resource from validated configuration.
    pub fn new(config: P2PSessionConfig) -> Self {
        let peers = config
            .agreement
            .roster
            .iter()
            .map(|peer| PeerState::new(*peer, *peer == config.local_peer_id))
            .collect();
        Self {
            config,
            state: P2PSessionState::WaitingForPeers,
            peers,
        }
    }

    /// Current public lifecycle state.
    pub fn state(&self) -> &P2PSessionState {
        &self.state
    }

    /// Agreed fixed-session configuration.
    pub fn agreement(&self) -> &P2PAgreement {
        &self.config.agreement
    }

    /// Returns the agreed start tick after it has been scheduled.
    pub fn start_tick(&self) -> Option<Tick> {
        match self.state {
            P2PSessionState::Starting { start_tick } | P2PSessionState::Running { start_tick } => {
                Some(start_tick)
            }
            _ => None,
        }
    }

    /// Returns true only after every roster member acknowledged the epoch and it was reached.
    pub fn is_running(&self) -> bool {
        matches!(self.state, P2PSessionState::Running { .. })
    }

    fn expected_remote_links(&self) -> u8 {
        (self.peers.len() - 1) as u8
    }

    fn local_index(&self) -> usize {
        self.peers
            .iter()
            .position(|peer| peer.peer_id == self.config.local_peer_id)
            .expect("validated P2P roster contains local peer")
    }

    fn peer_index(&self, peer_id: PeerId) -> Option<usize> {
        self.peers.iter().position(|peer| peer.peer_id == peer_id)
    }

    fn first_missing_peer(&self, connected: &[Entity]) -> Option<PeerId> {
        self.peers
            .iter()
            .find(|peer| {
                peer.peer_id != self.config.local_peer_id
                    && peer.link.is_none_or(|link| !connected.contains(&link))
            })
            .map(|peer| peer.peer_id)
    }

    fn receive(
        &mut self,
        peer_id: PeerId,
        message: P2PSessionMessage,
    ) -> Result<(), P2PSessionError> {
        let index = self
            .peer_index(peer_id)
            .ok_or(P2PSessionError::MessageFromUnexpectedPeer { peer: peer_id })?;
        match message {
            P2PSessionMessage::Hello(agreement) => {
                if let Some(field) = agreement_mismatch(&self.config.agreement, &agreement) {
                    return Err(P2PSessionError::AgreementMismatch {
                        peer: peer_id,
                        field,
                    });
                }
                self.peers[index].hello_received = true;
            }
            P2PSessionMessage::StartProposal {
                session_id,
                earliest_tick,
            } => {
                self.validate_start_message(peer_id, session_id, self.peers[index].hello_received)?;
                if let Some(first) = self.peers[index].proposal
                    && first != earliest_tick
                {
                    return Err(P2PSessionError::ConflictingStartProposal {
                        peer: peer_id,
                        first,
                        second: earliest_tick,
                    });
                }
                self.peers[index].proposal = Some(earliest_tick);
            }
            P2PSessionMessage::StartAcknowledgement {
                session_id,
                start_tick,
            } => {
                self.validate_start_message(peer_id, session_id, self.peers[index].hello_received)?;
                if let Some(first) = self.peers[index].acknowledgement
                    && first != start_tick
                {
                    return Err(P2PSessionError::ConflictingStartAcknowledgement {
                        peer: peer_id,
                        first,
                        second: start_tick,
                    });
                }
                self.peers[index].acknowledgement = Some(start_tick);
            }
        }
        Ok(())
    }

    fn validate_start_message(
        &self,
        peer: PeerId,
        session_id: P2PSessionId,
        hello_received: bool,
    ) -> Result<(), P2PSessionError> {
        if session_id != self.config.agreement.session_id {
            return Err(P2PSessionError::WrongSessionMessage { peer });
        }
        if !hello_received {
            return Err(P2PSessionError::StartBeforeHello { peer });
        }
        Ok(())
    }

    fn advance(&mut self, tick: Tick, timeline_synced: bool) -> Result<(), P2PSessionError> {
        loop {
            match self.state {
                P2PSessionState::Handshaking
                    if self.peers.iter().all(|peer| peer.hello_received) =>
                {
                    self.state = P2PSessionState::WaitingForTimeline;
                }
                P2PSessionState::WaitingForTimeline if timeline_synced => {
                    let local = self.local_index();
                    self.peers[local].proposal = Some(tick);
                    self.state = P2PSessionState::AgreeingStart;
                }
                P2PSessionState::AgreeingStart => {
                    if self.peers.iter().any(|peer| peer.proposal.is_none()) {
                        break;
                    }
                    let latest_proposal = self
                        .peers
                        .iter()
                        .filter_map(|peer| peer.proposal)
                        .max_by_key(|tick| tick.0)
                        .expect("the validated roster is non-empty");
                    let mut start_tick = latest_proposal;
                    start_tick += u32::from(self.config.agreement.start_delay_ticks);
                    let local = self.local_index();
                    self.peers[local].acknowledgement = Some(start_tick);

                    if self.peers.iter().any(|peer| peer.acknowledgement.is_none()) {
                        break;
                    }
                    for peer in &self.peers {
                        let acknowledgement = peer
                            .acknowledgement
                            .expect("checked all acknowledgements above");
                        if acknowledgement != start_tick {
                            return Err(P2PSessionError::StartAcknowledgementMismatch {
                                peer: peer.peer_id,
                                expected: start_tick,
                                actual: acknowledgement,
                            });
                        }
                    }
                    if tick >= start_tick {
                        return Err(P2PSessionError::StartEpochElapsed {
                            start_tick,
                            current_tick: tick,
                        });
                    }
                    self.state = P2PSessionState::Starting { start_tick };
                }
                P2PSessionState::Starting { start_tick } if tick >= start_tick => {
                    self.state = P2PSessionState::Running { start_tick };
                }
                _ => break,
            }
        }
        Ok(())
    }

    fn collect_outbound(&mut self, outbound: &mut SmallVec<[(Entity, P2PSessionMessage); 12]>) {
        let agreement = self.config.agreement.clone();
        let session_id = agreement.session_id;
        let local = self.local_index();
        let local_proposal = self.peers[local].proposal;
        let local_acknowledgement = self.peers[local].acknowledgement;
        for peer in &mut self.peers {
            let Some(link) = peer.link else {
                continue;
            };
            if !peer.hello_sent {
                outbound.push((link, P2PSessionMessage::Hello(agreement.clone())));
                peer.hello_sent = true;
            }
            if let Some(earliest_tick) = local_proposal
                && !peer.proposal_sent
            {
                outbound.push((
                    link,
                    P2PSessionMessage::StartProposal {
                        session_id,
                        earliest_tick,
                    },
                ));
                peer.proposal_sent = true;
            }
            if let Some(start_tick) = local_acknowledgement
                && peer.acknowledgement_sent != Some(start_tick)
            {
                outbound.push((
                    link,
                    P2PSessionMessage::StartAcknowledgement {
                        session_id,
                        start_tick,
                    },
                ));
                peer.acknowledgement_sent = Some(start_tick);
            }
        }
    }
}

/// Run condition for gameplay systems that require an agreed running roster.
pub fn p2p_session_running(session: Option<Res<P2PSession>>) -> bool {
    session.is_some_and(|session| session.is_running())
}

/// Triggered after every peer acknowledges the same future start tick.
#[derive(Event, Debug, Clone, Copy, PartialEq, Eq)]
pub struct P2PSessionStartScheduled {
    /// Agreed future tick.
    pub start_tick: Tick,
}

/// Triggered when the agreed start tick is reached.
#[derive(Event, Debug, Clone, Copy, PartialEq, Eq)]
pub struct P2PSessionStarted {
    /// Tick at which the session began running.
    pub start_tick: Tick,
}

/// Triggered when the configured peer-loss policy pauses the session.
#[derive(Event, Debug, Clone, Copy, PartialEq, Eq)]
pub struct P2PSessionPaused {
    /// Missing roster member.
    pub peer: PeerId,
}

/// Triggered when validation or peer loss aborts the session.
#[derive(Event, Debug, Clone, PartialEq)]
pub struct P2PSessionAborted {
    /// Fatal error retained by [`P2PSessionState::Aborted`].
    pub error: P2PSessionError,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum P2PSessionMessage {
    Hello(P2PAgreement),
    StartProposal {
        session_id: P2PSessionId,
        earliest_tick: Tick,
    },
    StartAcknowledgement {
        session_id: P2PSessionId,
        start_tick: Tick,
    },
}

/// Ordered reliable channel for the low-volume session handshake.
pub struct P2PSessionChannel;

/// Installs fixed-roster validation and the reliable session protocol.
///
/// Add this plugin after Lightyear's client/message plugins and before spawning P2P Link entities.
/// Insert one [`P2PSession`] resource before connecting the roster.
pub struct P2PSessionPlugin;

impl Plugin for P2PSessionPlugin {
    fn build(&self, app: &mut App) {
        app.add_channel::<P2PSessionChannel>(ChannelSettings {
            mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
            ..Default::default()
        })
        .add_direction(NetworkDirection::Bidirectional);
        app.register_message::<P2PSessionMessage>()
            .add_direction(NetworkDirection::Bidirectional);

        app.add_observer(abort_on_timeline_divergence);
        app.add_systems(
            PreUpdate,
            drive_session
                .after(MessageSystems::Receive)
                .after(NetworkTopologySystems::Update),
        );
    }
}

type P2PLinkQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        Option<&'static LocalId>,
        Option<&'static RemoteId>,
        Has<AuthenticatedPeerId>,
        Option<&'static mut MessageSender<P2PSessionMessage>>,
        Option<&'static mut MessageReceiver<P2PSessionMessage>>,
    ),
    (With<P2P>, With<Connected>),
>;

fn drive_session(
    mut commands: Commands,
    mut session: Option<ResMut<P2PSession>>,
    metadata: Res<NetworkingMetadata>,
    timeline: Res<LocalTimeline>,
    synced_timeline: Option<SyncedInputTimeline>,
    mut virtual_time: ResMut<Time<Virtual>>,
    mut links: P2PLinkQuery,
) {
    let Some(session) = session.as_deref_mut() else {
        return;
    };
    if matches!(
        session.state,
        P2PSessionState::Paused { .. } | P2PSessionState::Aborted { .. }
    ) {
        return;
    }

    let connected = match &metadata.mode {
        NetworkTopology::P2P {
            connected,
            declared_links,
        } => {
            let expected = session.expected_remote_links();
            if *declared_links > expected {
                abort_session(
                    session,
                    &mut virtual_time,
                    &mut commands,
                    P2PSessionError::DeclaredLinkCount {
                        declared: *declared_links,
                        expected,
                    },
                );
                return;
            }
            if *declared_links < expected || connected.len() < usize::from(expected) {
                if !matches!(session.state, P2PSessionState::WaitingForPeers) {
                    if let Some(peer) = session.first_missing_peer(connected) {
                        handle_peer_loss(session, &mut virtual_time, &mut commands, peer);
                    } else {
                        abort_session(
                            session,
                            &mut virtual_time,
                            &mut commands,
                            P2PSessionError::WrongTopology,
                        );
                    }
                }
                return;
            }
            connected
        }
        NetworkTopology::Undefined if matches!(session.state, P2PSessionState::WaitingForPeers) => {
            return;
        }
        NetworkTopology::Invalid(error) => {
            abort_session(
                session,
                &mut virtual_time,
                &mut commands,
                P2PSessionError::InvalidTopology(error.clone()),
            );
            return;
        }
        _ => {
            abort_session(
                session,
                &mut virtual_time,
                &mut commands,
                P2PSessionError::WrongTopology,
            );
            return;
        }
    };

    if let Err(error) = validate_connected_roster(session, connected, &mut links) {
        abort_session(session, &mut virtual_time, &mut commands, error);
        return;
    }

    'receive: for entity in connected {
        let Ok((_, _, remote_id, _, _, receiver)) = links.get_mut(*entity) else {
            abort_session(
                session,
                &mut virtual_time,
                &mut commands,
                P2PSessionError::MissingLink { link: *entity },
            );
            return;
        };
        let Some(remote_id) = remote_id else {
            continue;
        };
        let Some(mut receiver) = receiver else {
            continue;
        };
        for message in receiver.receive() {
            if let Err(error) = session.receive(remote_id.0, message) {
                abort_session(session, &mut virtual_time, &mut commands, error);
                break 'receive;
            }
        }
    }
    if matches!(session.state, P2PSessionState::Aborted { .. }) {
        return;
    }

    let previous = session.state.clone();
    if let Err(error) = session.advance(timeline.tick(), synced_timeline.is_some()) {
        abort_session(session, &mut virtual_time, &mut commands, error);
        return;
    }
    match (&previous, &session.state) {
        (_, P2PSessionState::Starting { start_tick })
            if !matches!(previous, P2PSessionState::Starting { .. }) =>
        {
            tracing::info!(?start_tick, "scheduled P2P session start");
            commands.trigger(P2PSessionStartScheduled {
                start_tick: *start_tick,
            });
        }
        (P2PSessionState::Starting { .. }, P2PSessionState::Running { start_tick }) => {
            tracing::info!(?start_tick, "P2P session started");
            commands.trigger(P2PSessionStarted {
                start_tick: *start_tick,
            });
        }
        _ => {}
    }

    let mut outbound = SmallVec::<[(Entity, P2PSessionMessage); 12]>::new();
    session.collect_outbound(&mut outbound);
    for (entity, message) in outbound {
        let Ok((_, _, _, _, sender, _)) = links.get_mut(entity) else {
            abort_session(
                session,
                &mut virtual_time,
                &mut commands,
                P2PSessionError::MissingLink { link: entity },
            );
            return;
        };
        let Some(mut sender) = sender else {
            abort_session(
                session,
                &mut virtual_time,
                &mut commands,
                P2PSessionError::MissingMessageSender { link: entity },
            );
            return;
        };
        sender.send::<P2PSessionChannel>(message);
    }
}

fn validate_connected_roster(
    session: &mut P2PSession,
    connected: &[Entity],
    links: &mut P2PLinkQuery,
) -> Result<(), P2PSessionError> {
    let mut observed: SmallVec<[(usize, Entity); MAX_ROSTER_SIZE]> = SmallVec::new();
    for entity in connected {
        let Ok((_, local_id, remote_id, authenticated, sender, _)) = links.get_mut(*entity) else {
            return Err(P2PSessionError::MissingLink { link: *entity });
        };
        let Some(local_id) = local_id else {
            return Err(P2PSessionError::MissingLocalId { link: *entity });
        };
        let Some(remote_id) = remote_id else {
            return Err(P2PSessionError::MissingRemoteId { link: *entity });
        };
        if local_id.0 != session.config.local_peer_id {
            return Err(P2PSessionError::LocalIdentityMismatch {
                link: *entity,
                expected: session.config.local_peer_id,
                actual: local_id.0,
            });
        }
        let Some(index) = session.peer_index(remote_id.0) else {
            return Err(P2PSessionError::UnexpectedPeer {
                link: *entity,
                peer: remote_id.0,
            });
        };
        if remote_id.0 == session.config.local_peer_id {
            return Err(P2PSessionError::UnexpectedPeer {
                link: *entity,
                peer: remote_id.0,
            });
        }
        if let Some((_, first)) = observed.iter().find(|(seen, _)| *seen == index) {
            return Err(P2PSessionError::DuplicatePeerLink {
                peer: remote_id.0,
                first: *first,
                second: *entity,
            });
        }
        if session.config.identity_policy == P2PIdentityPolicy::RequireAuthenticated
            && !authenticated
        {
            return Err(P2PSessionError::UnauthenticatedPeer {
                link: *entity,
                peer: remote_id.0,
            });
        }
        if sender.is_none() {
            return Err(P2PSessionError::MissingMessageSender { link: *entity });
        }
        observed.push((index, *entity));
    }

    for peer in session
        .peers
        .iter()
        .filter(|peer| peer.peer_id != session.config.local_peer_id)
    {
        if !observed
            .iter()
            .any(|(index, _)| session.peers[*index].peer_id == peer.peer_id)
        {
            return Err(P2PSessionError::PeerLost { peer: peer.peer_id });
        }
    }

    if matches!(session.state, P2PSessionState::WaitingForPeers) {
        for (index, entity) in observed {
            session.peers[index].link = Some(entity);
        }
        session.state = P2PSessionState::Handshaking;
    } else {
        for (index, entity) in observed {
            if session.peers[index].link != Some(entity) {
                return Err(P2PSessionError::PeerLost {
                    peer: session.peers[index].peer_id,
                });
            }
        }
    }
    Ok(())
}

fn agreement_mismatch(
    expected: &P2PAgreement,
    actual: &P2PAgreement,
) -> Option<P2PAgreementMismatch> {
    if expected.session_id != actual.session_id {
        Some(P2PAgreementMismatch::SessionId)
    } else if expected.roster != actual.roster {
        Some(P2PAgreementMismatch::Roster)
    } else if expected.configuration != actual.configuration {
        Some(P2PAgreementMismatch::Configuration)
    } else if expected.start_delay_ticks != actual.start_delay_ticks {
        Some(P2PAgreementMismatch::StartDelay)
    } else if expected.peer_loss_policy != actual.peer_loss_policy {
        Some(P2PAgreementMismatch::PeerLossPolicy)
    } else {
        None
    }
}

fn handle_peer_loss(
    session: &mut P2PSession,
    virtual_time: &mut Time<Virtual>,
    commands: &mut Commands,
    peer: PeerId,
) {
    virtual_time.pause();
    match session.config.agreement.peer_loss_policy {
        P2PPeerLossPolicy::Abort => {
            abort_session(
                session,
                virtual_time,
                commands,
                P2PSessionError::PeerLost { peer },
            );
        }
        P2PPeerLossPolicy::Pause => {
            session.state = P2PSessionState::Paused { peer };
            commands.trigger(P2PSessionPaused { peer });
        }
    }
}

fn abort_session(
    session: &mut P2PSession,
    virtual_time: &mut Time<Virtual>,
    commands: &mut Commands,
    error: P2PSessionError,
) {
    if matches!(session.state, P2PSessionState::Aborted { .. }) {
        return;
    }
    tracing::error!(%error, "aborting P2P session");
    virtual_time.pause();
    session.state = P2PSessionState::Aborted {
        error: error.clone(),
    };
    commands.trigger(P2PSessionAborted { error });
}

fn abort_on_timeline_divergence(
    trigger: On<P2PTimelineDiverged>,
    mut session: Option<ResMut<P2PSession>>,
    mut virtual_time: ResMut<Time<Virtual>>,
    mut commands: Commands,
) {
    let Some(session) = session.as_deref_mut() else {
        return;
    };
    abort_session(
        session,
        &mut virtual_time,
        &mut commands,
        P2PSessionError::TimelineDiverged {
            limiting_link: trigger.limiting_link,
            lead: trigger.lead,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Resource, Default)]
    struct ValidationResult(Option<Result<(), P2PSessionError>>);

    fn peer(id: u64) -> PeerId {
        PeerId::Entity(id)
    }

    fn config(local: u64) -> P2PSessionConfig {
        P2PSessionConfig::new(
            P2PSessionId::from_u128(7),
            peer(local),
            [peer(0), peer(1), peer(2)],
            P2PConfigFingerprint::from_u64(11),
        )
        .unwrap()
        .with_start_delay_ticks(10)
        .with_identity_policy(P2PIdentityPolicy::TrustLinkIdentity)
    }

    fn begin_handshake(session: &mut P2PSession) {
        let mut link_id = 1;
        for peer in &mut session.peers {
            if peer.peer_id != session.config.local_peer_id {
                peer.link = Some(Entity::from_raw_u32(link_id).unwrap());
                link_id += 1;
            }
        }
        session.state = P2PSessionState::Handshaking;
    }

    fn validate_test_links(
        mut session: ResMut<P2PSession>,
        entities: Query<Entity, (With<P2P>, With<Connected>)>,
        mut links: P2PLinkQuery,
        mut result: ResMut<ValidationResult>,
    ) {
        let connected: SmallVec<[Entity; MAX_ROSTER_SIZE]> = entities.iter().collect();
        result.0 = Some(validate_connected_roster(
            &mut session,
            &connected,
            &mut links,
        ));
    }

    fn validation_app(authenticated: bool) -> App {
        let mut app = App::new();
        app.insert_resource(P2PSession::new(
            P2PSessionConfig::new(
                P2PSessionId::from_u128(7),
                peer(0),
                [peer(0), peer(1)],
                P2PConfigFingerprint::from_u64(11),
            )
            .unwrap(),
        ));
        app.init_resource::<ValidationResult>();
        let entity = app
            .world_mut()
            .spawn((
                P2P,
                LocalId(peer(0)),
                RemoteId(peer(1)),
                Connected,
                MessageSender::<P2PSessionMessage>::default(),
            ))
            .id();
        if authenticated {
            app.world_mut()
                .entity_mut(entity)
                .insert(AuthenticatedPeerId);
        }
        app.add_systems(bevy_app::Update, validate_test_links);
        app
    }

    #[test]
    fn validates_fixed_roster_configuration() {
        assert_eq!(
            P2PSessionConfig::new(
                P2PSessionId::from_u128(1),
                peer(0),
                [peer(0)],
                P2PConfigFingerprint::from_u64(1),
            )
            .unwrap_err(),
            P2PSessionConfigError::RosterSize(1)
        );
        assert_eq!(
            P2PSessionConfig::new(
                P2PSessionId::from_u128(1),
                peer(0),
                [peer(0), peer(1), peer(1)],
                P2PConfigFingerprint::from_u64(1),
            )
            .unwrap_err(),
            P2PSessionConfigError::DuplicatePeer(peer(1))
        );
        assert_eq!(
            P2PSessionConfig::new(
                P2PSessionId::from_u128(1),
                peer(3),
                [peer(0), peer(1)],
                P2PConfigFingerprint::from_u64(1),
            )
            .unwrap_err(),
            P2PSessionConfigError::LocalPeerMissing(peer(3))
        );
    }

    #[test]
    fn authenticated_identity_is_required_by_default() {
        let mut unauthenticated = validation_app(false);
        unauthenticated.update();
        assert!(matches!(
            unauthenticated
                .world()
                .resource::<ValidationResult>()
                .0
                .as_ref()
                .unwrap(),
            Err(P2PSessionError::UnauthenticatedPeer { peer: actual, .. })
                if *actual == peer(1)
        ));

        let mut authenticated = validation_app(true);
        authenticated.update();
        assert_eq!(
            authenticated.world().resource::<ValidationResult>().0,
            Some(Ok(()))
        );
    }

    #[test]
    fn all_to_all_proposals_choose_one_future_start_tick() {
        let mut session = P2PSession::new(config(0));
        begin_handshake(&mut session);
        let agreement = session.config.agreement.clone();
        session
            .receive(peer(1), P2PSessionMessage::Hello(agreement.clone()))
            .unwrap();
        session
            .receive(peer(2), P2PSessionMessage::Hello(agreement))
            .unwrap();

        session.advance(Tick(5), false).unwrap();
        assert_eq!(session.state, P2PSessionState::WaitingForTimeline);
        session.advance(Tick(6), true).unwrap();
        assert_eq!(session.state, P2PSessionState::AgreeingStart);

        for (remote, proposal) in [(peer(1), Tick(8)), (peer(2), Tick(7))] {
            session
                .receive(
                    remote,
                    P2PSessionMessage::StartProposal {
                        session_id: session.config.agreement.session_id,
                        earliest_tick: proposal,
                    },
                )
                .unwrap();
        }
        session.advance(Tick(9), true).unwrap();
        let expected_start = Tick(18);
        for remote in [peer(1), peer(2)] {
            session
                .receive(
                    remote,
                    P2PSessionMessage::StartAcknowledgement {
                        session_id: session.config.agreement.session_id,
                        start_tick: expected_start,
                    },
                )
                .unwrap();
        }
        session.advance(Tick(10), true).unwrap();
        assert_eq!(
            session.state,
            P2PSessionState::Starting {
                start_tick: expected_start
            }
        );
        session.advance(expected_start, true).unwrap();
        assert_eq!(
            session.state,
            P2PSessionState::Running {
                start_tick: expected_start
            }
        );
    }

    #[test]
    fn rejects_agreement_and_start_acknowledgement_mismatches() {
        let mut session = P2PSession::new(config(0));
        begin_handshake(&mut session);
        let mut mismatched = session.config.agreement.clone();
        mismatched.configuration = P2PConfigFingerprint::from_u64(99);
        assert_eq!(
            session
                .receive(peer(1), P2PSessionMessage::Hello(mismatched))
                .unwrap_err(),
            P2PSessionError::AgreementMismatch {
                peer: peer(1),
                field: P2PAgreementMismatch::Configuration,
            }
        );

        let agreement = session.config.agreement.clone();
        for remote in [peer(1), peer(2)] {
            session
                .receive(remote, P2PSessionMessage::Hello(agreement.clone()))
                .unwrap();
        }
        session.advance(Tick(5), true).unwrap();
        for (remote, proposal) in [(peer(1), Tick(8)), (peer(2), Tick(7))] {
            session
                .receive(
                    remote,
                    P2PSessionMessage::StartProposal {
                        session_id: session.config.agreement.session_id,
                        earliest_tick: proposal,
                    },
                )
                .unwrap();
        }
        session.advance(Tick(9), true).unwrap();
        for (remote, start_tick) in [(peer(1), Tick(19)), (peer(2), Tick(18))] {
            session
                .receive(
                    remote,
                    P2PSessionMessage::StartAcknowledgement {
                        session_id: session.config.agreement.session_id,
                        start_tick,
                    },
                )
                .unwrap();
        }
        assert_eq!(
            session.advance(Tick(10), true).unwrap_err(),
            P2PSessionError::StartAcknowledgementMismatch {
                peer: peer(1),
                expected: Tick(18),
                actual: Tick(19),
            }
        );
    }

    fn apply_peer_loss(
        mut commands: Commands,
        mut session: ResMut<P2PSession>,
        mut virtual_time: ResMut<Time<Virtual>>,
    ) {
        handle_peer_loss(&mut session, &mut virtual_time, &mut commands, peer(1));
    }

    #[test]
    fn peer_loss_policy_pauses_virtual_time_and_session() {
        let mut app = App::new();
        let session_config = config(0).with_peer_loss_policy(P2PPeerLossPolicy::Pause);
        app.insert_resource(Time::<Virtual>::default());
        app.insert_resource(P2PSession::new(session_config));
        app.add_systems(bevy_app::Update, apply_peer_loss);

        app.update();

        assert!(app.world().resource::<Time<Virtual>>().is_paused());
        assert_eq!(
            app.world().resource::<P2PSession>().state(),
            &P2PSessionState::Paused { peer: peer(1) }
        );
    }
}
