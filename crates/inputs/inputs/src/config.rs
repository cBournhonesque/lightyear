use core::marker::PhantomData;
use core::time::Duration;

use bevy_ecs::resource::Resource;
use bevy_reflect::Reflect;

/// Configuration for one input type
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
    /// Each message repeats the inputs of the last `packet_redundancy` send attempts.
    /// Higher values cost bandwidth per message but recover from longer loss bursts.
    pub packet_redundancy: u16,
    /// Time between sending input messages.
    ///
    /// `Duration::default()` (zero) means that we send a message every frame.
    pub send_interval: Duration,
    /// If true, the actions won't be rolled back when a rollback happens.
    ///
    /// This can be useful for actions that should not be replayed, for example settings-related actions.
    pub ignore_rollbacks: bool,
    /// If True, the server will rebroadcast a client's inputs to all other clients.
    ///
    /// It is useful for a client to have access to other client's inputs to be able
    /// to predict their actions.
    pub rebroadcast_inputs: bool,
    pub marker: PhantomData<A>,
}

impl<A> Copy for InputConfig<A> {}

impl<A> Clone for InputConfig<A> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<A> Default for InputConfig<A> {
    fn default() -> Self {
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
}
