use crate::client::Client;
use bevy_ecs::prelude::*;
use bevy_reflect::Reflect;
use smallvec::SmallVec;

/// Participation state of a direct peer Link in a P2P session.
///
/// A P2P Link remains a [`Client`] Link so that it can reuse the existing connection,
/// messaging, input, and prediction pipelines. The component is immutable: replace it to move a
/// Link between states so that the cached network topology observes every transition.
#[derive(Component, Default, Debug, Clone, Copy, PartialEq, Eq, Reflect)]
#[component(immutable)]
#[require(Client)]
pub enum P2P {
    /// A declared P2P Link that is not part of a start barrier or running session.
    ///
    /// Inactive Links do not activate a P2P [`NetworkTopology`](crate::network_topology::NetworkTopology).
    #[default]
    Inactive,
    /// A Link whose peer has been admitted to a barrier but is not playing yet.
    ///
    /// A candidate is a **start candidate** while this application is forming a new cohort, and a
    /// **join candidate** while it is admitting a peer to a session that is already running; see
    /// [`P2PSessionPhase`]. Either way it is not part of the deterministic world yet, so it is
    /// deliberately not exposed through the cached
    /// [`NetworkTopology`](crate::network_topology::NetworkTopology). Systems that participate in
    /// startup synchronization should query this component directly.
    Candidate,
    /// A Link that has crossed the barrier. While connected, it is exposed through
    /// [`NetworkTopology::P2P`](crate::network_topology::NetworkTopology::P2P).
    Joined,
}

/// Lifecycle of the deterministic P2P session on this application.
///
/// The session crate sits *above* the layers that need this information — the cached topology,
/// timeline synchronization, and input routing — so it publishes its lifecycle through this
/// resource instead. [`P2P`] describes one Link; this describes the application.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub enum P2PSessionPhase {
    /// No deterministic session on this application.
    #[default]
    Stopped,
    /// The start barrier is in flight: this application is one of the peers forming a new cohort.
    ///
    /// Every declared candidate is a **start candidate**. The cohort is not playing yet, and the
    /// barrier aligns all participants onto the slowest starter's tick.
    Starting,
    /// This application has been admitted to a session that is already running, as a **join
    /// candidate**.
    ///
    /// It is not playing in that session yet, and it must not pace it: it follows the cohort's
    /// tick instead of asking the cohort to follow its own, and it stays out of the peers'
    /// confirmed-input frontier until it is admitted.
    Joining,
    /// This application is a started peer: it plays in the session.
    ///
    /// Its Links are [`P2P::Joined`], it paces against them, and it owns the session-wide input
    /// delay.
    Active,
}

/// A deterministic P2P session in progress on this application, with its membership cached.
///
/// The [`NetworkTopology`](crate::network_topology::NetworkTopology) projection owns this: it is
/// recomputed once per frame from the declared [`P2P`] Links, so every consumer of the cached
/// topology reads the same membership snapshot instead of running its own query.
///
/// Connectivity is deliberately **not** a single filter here: readiness and input routing ask
/// different questions about the same declared peer, and both answers matter. The candidate sets
/// are partitioned by connectivity rather than nested, so no peer appears twice; a caller that
/// needs every candidate uses [`declared_candidates`](Self::declared_candidates).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct P2PRoster {
    /// Where this application is in the session lifecycle.
    pub phase: P2PSessionPhase,
    /// Peers that are playing: [`P2P::Joined`] and currently connected.
    ///
    /// This is the deterministic roster.
    pub started: SmallVec<[Entity; 4]>,
    /// Admitted peers with a live Link, which can carry messages right now.
    ///
    /// These are the peers input routing can reach, and the ones a barrier can exchange session
    /// messages with.
    pub connected_candidates: SmallVec<[Entity; 4]>,
    /// Admitted peers whose Link is declared but not connected yet.
    ///
    /// A peer belongs here between its Link being admitted and that Link coming up. Readiness must
    /// include one: a barrier waits for the whole declared roster, and the input delay must already
    /// account for the Link that peer will use.
    pub unconnected_candidates: SmallVec<[Entity; 4]>,
}

impl P2PRoster {
    /// Whether this application is a started peer of this session.
    #[inline]
    pub fn is_started_peer(&self) -> bool {
        matches!(self.phase, P2PSessionPhase::Active)
    }

    /// Every admitted peer that is not playing yet, connected or not.
    ///
    /// The union of [`connected_candidates`](Self::connected_candidates) and
    /// [`unconnected_candidates`](Self::unconnected_candidates), in local [`Entity`] order.
    pub fn declared_candidates(&self) -> SmallVec<[Entity; 8]> {
        let mut declared: SmallVec<[Entity; 8]> = SmallVec::new();
        declared.extend(self.connected_candidates.iter().copied());
        declared.extend(self.unconnected_candidates.iter().copied());
        declared.sort_unstable_by_key(|entity| entity.index_u32());
        declared
    }

    /// Whether any peer is declared for this session but is not playing yet.
    ///
    /// This is the question the topology projection asks: a declared cohort that has not started
    /// playing has no ready roster.
    #[inline]
    pub fn has_declared_candidates(&self) -> bool {
        !self.connected_candidates.is_empty() || !self.unconnected_candidates.is_empty()
    }

    /// The Links that can carry gameplay input for this application.
    ///
    /// That is the started peers plus the admitted peers that are connected: a peer which is still
    /// catching up must receive the session's inputs, and it is the only route an application that
    /// is not playing yet has. The unconnected candidates are excluded because they have no session
    /// to carry anything over.
    ///
    /// Stays inline for the supported player counts.
    pub fn input_links(&self) -> SmallVec<[Entity; 8]> {
        let mut links: SmallVec<[Entity; 8]> = SmallVec::new();
        links.extend(self.started.iter().copied());
        links.extend(self.connected_candidates.iter().copied());
        links
    }

    /// A roster of started peers with no candidates.
    ///
    /// Convenience for applications and tests that know the started set directly rather than
    /// deriving it from a set of Links.
    pub fn from_started_links(links: impl IntoIterator<Item = Entity>) -> Self {
        let mut roster = Self {
            phase: P2PSessionPhase::Active,
            ..Default::default()
        };
        roster.started.extend(links);
        roster
            .started
            .sort_unstable_by_key(|entity| entity.index_u32());
        roster
    }

    /// Classify this application's [`P2P`] Links into a roster.
    ///
    /// `links` yields each declared Link as `(entity, state, is_connected)`. The state is taken by
    /// value because it is `Copy`, which lets callers pass it straight out of a `Ref` query.
    ///
    /// Every set is sorted by local [`Entity`] ID, so the result does not depend on query iteration
    /// order.
    pub fn from_links(
        phase: P2PSessionPhase,
        links: impl IntoIterator<Item = (Entity, P2P, bool)>,
    ) -> Self {
        let mut roster = Self {
            phase,
            ..Default::default()
        };
        for (entity, state, connected) in links {
            match state {
                // A disconnected started peer is not part of the deterministic world right now.
                P2P::Joined if connected => roster.started.push(entity),
                P2P::Candidate if connected => roster.connected_candidates.push(entity),
                P2P::Candidate => roster.unconnected_candidates.push(entity),
                P2P::Inactive | P2P::Joined => {}
            }
        }
        roster
            .started
            .sort_unstable_by_key(|entity| entity.index_u32());
        roster
            .connected_candidates
            .sort_unstable_by_key(|entity| entity.index_u32());
        roster
            .unconnected_candidates
            .sort_unstable_by_key(|entity| entity.index_u32());
        roster
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_sets_separate_started_peers_from_candidates() {
        let mut world = World::new();
        let inactive = world.spawn_empty().id();
        let started = world.spawn_empty().id();
        let disconnected_started = world.spawn_empty().id();
        let candidate = world.spawn_empty().id();
        // Declared but not connected: a barrier still has to wait for it.
        let pending_candidate = world.spawn_empty().id();

        let sets = P2PRoster::from_links(
            P2PSessionPhase::Active,
            [
                (inactive, P2P::Inactive, true),
                (started, P2P::Joined, true),
                (disconnected_started, P2P::Joined, false),
                (candidate, P2P::Candidate, true),
                (pending_candidate, P2P::Candidate, false),
            ],
        );

        assert_eq!(sets.started.as_slice(), &[started]);
        // The candidate sets partition the declared peers: each appears exactly once.
        assert_eq!(sets.connected_candidates.as_slice(), &[candidate]);
        assert_eq!(sets.unconnected_candidates.as_slice(), &[pending_candidate]);
        assert!(sets.has_declared_candidates());
        // The union is what readiness wants, sorted so it cannot depend on iteration order.
        assert_eq!(
            sets.declared_candidates().as_slice(),
            &[candidate, pending_candidate]
        );
    }

    #[test]
    fn link_sets_are_empty_without_declared_links() {
        let sets = P2PRoster::from_links(P2PSessionPhase::Active, []);
        assert!(sets.started.is_empty());
        assert!(!sets.has_declared_candidates());
        assert!(sets.connected_candidates.is_empty());
    }
}
