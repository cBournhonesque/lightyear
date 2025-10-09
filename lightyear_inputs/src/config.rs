use core::marker::PhantomData;
use core::time::Duration;

use bevy_ecs::resource::Resource;
use bevy_reflect::Reflect;

// TODO: add builder functions on InputPlugin to add
#[derive(Debug, Reflect, Resource)]
pub struct InputConfig<A> {
    #[cfg(feature = "interpolation")]
    /// If enabled, the client will send the interpolation_delay to the server so that the server
    /// can apply lag compensation when the predicted client is shooting at interpolated enemies.
    ///
    /// See: <https://developer.valvesoftware.com/wiki/Lag_Compensation>
    pub lag_compensation: bool,
    /// How many consecutive packets losses do we want to handle?
    /// This is used to compute the redundancy of the input messages.
    /// For instance, a value of 3 means that each input packet will contain the inputs for all the ticks
    ///  for the 3 last packets.
    pub packet_redundancy: u16,
    /// How often do we send input messages to the server?
    /// Duration::default() means that we will send input messages every frame.
    pub send_interval: Duration,
    /// If true, the actions won't be rolled back when a rollback happens.
    ///
    /// This can be useful for actions that should not be replayed, for example settings-related actions.
    pub ignore_rollbacks: bool,
    /// If True, the server will rebroadcast a client's inputs to all other clients.
    ///
    /// It could be useful for a client to have access to other client's inputs to be able
    /// to predict their actions
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

/// Input config shared across all Action types.
/// Used to avoid creating some systems multiple times
#[derive(Default, Resource)]
pub(crate) struct SharedInputConfig {
    pub(crate) reset_last_confirmed_system_added: bool,
}
