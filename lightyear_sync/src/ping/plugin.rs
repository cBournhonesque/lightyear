use crate::ping::manager::PingManager;
use crate::ping::message::{Ping, Pong};
use crate::ping::PingChannel;
use bevy::platform_support::time::Instant;
use bevy::prelude::*;
use lightyear_core::time::{SetTickDuration, TickDelta};
use lightyear_link::Link;
use lightyear_messages::plugin::MessageSet;
use lightyear_messages::receive::MessageReceiver;
use lightyear_messages::send::MessageSender;
use lightyear_transport::prelude::Transport;

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum PingSet {
    /// Receive messages from the Link and buffer them into the ChannelReceivers
    Receive,
    /// Flush the messages buffered in the ChannelSenders to the Link
    Send,
}

pub struct PingPlugin;

impl PingPlugin {
    fn receive(
        real_time: Res<Time<Real>>,
        mut query: Query<(&mut Link, &mut PingManager, &mut MessageReceiver<Ping>, &mut MessageReceiver<Pong>)>,
    ) {
        query.par_iter_mut().for_each(|(mut link, mut m, mut ping_receiver, mut pong_receiver)| {
            // update
            m.update(&real_time);

            // receive pings
            ping_receiver.receive().for_each(|ping| {
                m.buffer_pending_pong(&ping, real_time.elapsed());
            });
            // receive pongs
            pong_receiver.receive().for_each(|pong| {
                // process the pong
                m.process_pong(&pong, real_time.elapsed());
            });

            link.stats.rtt = m.rtt();
            link.stats.jitter = m.jitter();
        })

    }

    /// Send pings/pongs to the remote
    /// We modify the pongs that were buffered so that we can write the correct
    /// time spent between PostUpdate and PreUpdate
    fn send(
        real_time: Res<Time<Real>>,
        fixed_time: Res<Time<Fixed>>,
        mut query: Query<(&mut PingManager, &mut MessageSender<Ping>, &mut MessageSender<Pong>)>,
    ) {
        let now = Instant::now();
        query.par_iter_mut().for_each(|(mut m, mut ping_sender, mut pong_sender)| {
            let Some(frame_start) = real_time.last_update() else {
                return
            };
            // send the pings
            if let Some(ping) = m.maybe_prepare_ping(real_time.elapsed()) {
                ping_sender.send::<PingChannel>(ping);
            }
            // prepare the pong messages with the correct send time
            m
            .take_pending_pongs()
            .into_iter()
            .for_each(|(mut pong, ping_receive_time)| {
                pong.frame_time = TickDelta::from_duration(now - frame_start, m.tick_duration).into();

                // TODO: maybe include the tick + overstep in every packet?
                // TODO: how to use the overstep?
                // pong.overstep = fixed_time.overstep_fraction();
                pong_sender.send::<PingChannel>(pong);
            });
        })
    }

    pub(crate) fn update_tick_duration(
        trigger: Trigger<SetTickDuration>,
        mut query: Query<&mut PingManager>,
    ) {
        if let Ok(mut ping_manager) = query.get_mut(trigger.target()) {
            ping_manager.tick_duration = trigger.0;
        }
    }
}


impl Plugin for PingPlugin {
    fn build(&self, app: &mut App) {

        // NOTE: the Transport's PacketBuilder needs accurate LinkStats to function correctly.
        //   Theoretically anything can modify the LinkStats but in practice it's done in the PingManager
        //   so we make the Transport require a PingManager.
        //   Maybe we should error if TransportPlugin is added without PingPlugin?
        app.register_required_components::<Transport, PingManager>();

        app.configure_sets(PreUpdate, (MessageSet::Receive, PingSet::Receive).chain());
        app.configure_sets(PostUpdate,  (PingSet::Send, MessageSet::Send).chain());
        app.add_systems(PreUpdate, Self::receive.in_set(PingSet::Receive));
        app.add_systems(PostUpdate, Self::send.in_set(PingSet::Send));

        app.add_observer(Self::update_tick_duration);
    }

    fn finish(&self, app: &mut App) {
        // todo!("Add ping and pong here?")
    }
}
