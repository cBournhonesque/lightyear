use core::marker::PhantomData;
use core::time::Duration;

use bevy_ecs::resource::Resource;
use bevy_reflect::Reflect;

/// Tuning for one input type's client↔server exchange.
///
/// Obtain with [`InputConfig::new`] and chain the `with_*` builders, or use a
/// struct literal with `..Default::default()`.
#[derive(Debug, Reflect, Resource)]
pub struct InputConfig<A> {
    #[cfg(feature = "interpolation")]
    /// If enabled, the client will send the interpolation_delay to the server so that the server
    /// can apply lag compensation when the predicted client is shooting at interpolated enemies.
    ///
    /// See: <https://developer.valvesoftware.com/wiki/Lag_Compensation>
    pub lag_compensation: bool,
    /// How many consecutive packet losses to survive.
    ///
    /// Each message repeats the inputs for the last `packet_redundancy` send
    /// windows (see `send_interval`). Higher values cost bandwidth per message
    /// but recover from longer loss bursts. `0` disables history: messages then
    /// carry only `end_tick`, which still advances the receiver's confirmed
    /// frontier as a keepalive.
    pub packet_redundancy: u16,
    /// Minimum time between input messages.
    ///
    /// `Duration::default()` (zero) means one message per frame. Larger values
    /// batch more ticks into each message: the covered window scales as
    /// `(send_interval / tick_duration + 1) * packet_redundancy` ticks.
    pub send_interval: Duration,
    /// If true, the actions won't be rolled back when a rollback happens.
    ///
    /// This can be useful for actions that should not be replayed, for example settings-related actions.
    pub ignore_rollbacks: bool,
    /// If True, the server will rebroadcast a client's inputs to all other clients.
    ///
    /// It could be useful for a client to have access to other client's inputs to be able
    /// to predict their actions.
    ///
    /// This is the single resource both sides read: `ServerInputPlugin` stamps
    /// this flag from its `rebroadcast_inputs` builder field (inserting a
    /// default `InputConfig` first when the app never added one).
    pub rebroadcast_inputs: bool,
    pub marker: PhantomData<A>,
}

impl<A> Copy for InputConfig<A> {}

impl<A> Clone for InputConfig<A> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<A> InputConfig<A> {
    /// Default config; chain `with_*` to tune.
    pub fn new() -> Self {
        InputConfig {
            #[cfg(feature = "interpolation")]
            lag_compensation: false,
            packet_redundancy: 5,
            send_interval: Duration::default(),
            ignore_rollbacks: false,
            rebroadcast_inputs: false,
            marker: PhantomData,
        }
    }

    /// See [`InputConfig::packet_redundancy`].
    pub fn with_packet_redundancy(mut self, packet_redundancy: u16) -> Self {
        self.packet_redundancy = packet_redundancy;
        self
    }

    /// See [`InputConfig::send_interval`].
    pub fn with_send_interval(mut self, send_interval: Duration) -> Self {
        self.send_interval = send_interval;
        self
    }

    /// See [`InputConfig::ignore_rollbacks`].
    pub fn with_ignore_rollbacks(mut self, ignore_rollbacks: bool) -> Self {
        self.ignore_rollbacks = ignore_rollbacks;
        self
    }

    /// See [`InputConfig::rebroadcast_inputs`].
    pub fn with_rebroadcast_inputs(mut self, rebroadcast_inputs: bool) -> Self {
        self.rebroadcast_inputs = rebroadcast_inputs;
        self
    }

    /// See [`InputConfig::lag_compensation`].
    #[cfg(feature = "interpolation")]
    pub fn with_lag_compensation(mut self, lag_compensation: bool) -> Self {
        self.lag_compensation = lag_compensation;
        self
    }
}

impl<A> Default for InputConfig<A> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_sets_fields() {
        let config = InputConfig::<u8>::new()
            .with_packet_redundancy(2)
            .with_send_interval(Duration::from_millis(50))
            .with_ignore_rollbacks(true)
            .with_rebroadcast_inputs(true);
        assert_eq!(config.packet_redundancy, 2);
        assert_eq!(config.send_interval, Duration::from_millis(50));
        assert!(config.ignore_rollbacks);
        assert!(config.rebroadcast_inputs);

        let default = InputConfig::<u8>::default();
        assert_eq!(default.packet_redundancy, 5);
        assert!(!default.rebroadcast_inputs);
    }
}
