use bevy::prelude::{Reflect, Resource};
use core::marker::PhantomData;
use core::time::Duration;

// TODO: make this a component ?
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
            packet_redundancy: 10,
            send_interval: Duration::default(),
            rebroadcast_inputs: false,
            marker: PhantomData,
        }
    }
}
