# Bandwidth management

By default, lightyear sends all messages (created by the user, or messages created from replication updates)
every `send_interval` (this interval is configurable) without any regard for the bandwidth available to the client.

But in some situations you might want to limit the bandwidth used by the client or the server, for example to limit
server traffic costs, or because the client's connection cannot handle a very high bandwidth.

This page will explain how to do that. There are several options to choose from.

## Limiting the number of replication objects

The simplest thing you can do is to carefully choose which entities and components you need to replicate.
For example, rendering-related components (particles, assets, etc.) do not need to spawned on the server and replicated to the client.
They can be created on the client and only the necessary information (position, rotation, etc.) can be replicated.

This also saves CPU costs on the server.

## Updating the send interval

Another thing you can do is to update the `send_interval` of the client or server. This will reduce the number of times
the `SystemSet::Send` systems will run.
This `SystemSet` is responsible for aggregating all the messages that were buffered and are ready to send, as well as generating all the
replication-messages (entity-actions, entity-updates) that should be sent.

NOTE: Currently `lightyear` expects `send_interval` to be 0 on the client (i.e. the client sends all updates immediately) to manage client inputs properly.

This will also reduce the CPU usage of the server as it runs the replication-send logic less often.


## TODO: Updating the replication rate per replication group

You can also override the replication rate per replication group. 
For some entities it might not be important to run replication at a very high rate, so you can reduce the rate for those entities.

NOTE: this is currently not possible


## Prioritizing replication groups

Even so, there might be situations where you have more messages to send than the bandwidth available to you.
In that case you can set a **priority** to indicate which messages are important and should be sent first.

Every time the server (or client) is ready to send messages, it will first:
- aggregate the list of messages that should be sent
- then sort them by priority. The priority is computed with the formula `channel_priority * message_priority`.
- it will send messages in order of priority until all the bandwidth is used
- it will then discard the remaining messages
  - note that this means that discarded messages via an unreliable channel will simply **not be sent**
  - for entity updates, we still try to send an update until the remote world is consistent with the local world, so we will keep trying sending updates until we receive an ack from the remote that
    it received the updates.

Only the relative priority values matter, not their absolute value: an entity with priority 10 will be replicated twice as often as an entity with priority 5.

To avoid having some replication groups entities be starved of updates (because their priority is always too low), we do **priority accumulation**:
- every send_interval, we accumulate the priority of all messages: `accumulated_priority += priority`
- if a replication groups successfully sends an update or an action, we reset the accumulated priority to 0. (note that it's not guaranteed that the message was received by the remote, just that the message was sent)
- for reliable channels, we also keep accumulating the priority until we receive an ack from the remote that the message was successfully received