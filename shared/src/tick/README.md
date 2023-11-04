
Requirements:

- Server:
  - server has a global tick that is the frequency at which we call the main server netcode loop (update/receive/send)
  - a fixed timestep schedule with the tick-rate can be used to set the tick rate?
  - maybe we can do it in bevy via the SharedPlugin, since bevy already has good time handling?
   
  - maybe just do IncrementTick in the main receive loop?


- Client:
  - client has a global tick that is the frequency at which we call the main client netcode loop (update/receive/send)
  - client tick is ahead of the server tick by RTT/2 + small-buffer
    - so that if a player does an input at tick 200, and rtt is 4 ticks, then the server is currently at tick 198
      and they will receive the command roughly at tick 200 (after rtt/2)
    - adjust client time so that the client's tick is roughly = server_tick + RTT/2 + small_buffer

  - 


- TickBuffer:
  - we want to store packets (messages?) in a buffer, each packet associated with the tick it was sent at
  - send side:
    - we need to add tick_id to the packet (or the message)? Maybe this can be associated with the channel?
  - receive side:
    - we add the messages in a ReadyBuffer<Tick, Message> and only pop them when the tick is reached
    - then we want to read it on the exact same corresponding tick as when it was emitted (for example for client inputs)