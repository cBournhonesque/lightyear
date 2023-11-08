
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


- TimeManager: shared
  - handles updating global time
  - GLOBAL
- TickManager: separate implementation in server/client
  - server: just deduce tick from time
  - client: deduce tick from sync process (receiving ping/pong messages with server)
  - GLOBAL?
- PingManager: shared
  - maybe separate implementation client/server
  - how to send ping/pong with remote.
  - track timer to send ping
  - ONE PER CONNECTION
- SyncManager: client only
  - 

TODO:
- run the network update/receive/send function INSIDE each event of fixed update, in case the tick boundary is between some fixed update action


Do I even need to keep track of which server/client tick I am at?
Let's say I run the server and physics at 64fixed-update.
The only reason I would like ticks is for client prediction:
- client sent inputs at ticks C200, C202. It is simulating at tick C205. RTT ~ 10 ticks. Latest server update it received was for tick C195. 
- Server is currently at tick S199. At tick S200, it receives the client input from C200. It reconciles with other stuff and sends back the result for tick 200
- When client reaches tick C210 (server is at S205), it receives the update for tick C200
- It has to recompute the simulation from tick C200 to C210, starting from the new state sent by server, but re-applying his input at ticks 202.

Ticks give us:
- all actions done during one tick are viewed as simultaneous.

Client prediction:
- client needs to a copy of the inputs it sent to server between latest server update and current time. Can put them in an ordered buffer.
  The updates for a given system run will be the same
- server needs to know that the input from client at 'tick' 200 was really sent at tick 200, and starts processing at tick 200 (stores in a buffer beforehand)
- we could:
  - increment tick/counter by 1 at every fixed-update run

Snapshot Interpolation:
- For non-predicted entities, we keep a buffer so that we can keep always have at least 2 snapshots received from the server
- and then we interpolate between them


There will be time where we run multiple times physics+server, physics+server:
- pros: lets us send updates of the physics in the middle of fixed-updates updates
- cons: can send many packets very quickly.

