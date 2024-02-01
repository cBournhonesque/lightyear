# Priority

A simple example that shows how you can specify which messages/channels/entities have priority over others.
In case the bandwidth quota is reached, lightyear will only send the messages with the highest priority, up to the quota.

To not starve lower priority entities, their priority is accumulated over time, so that they can eventually be sent.

In this example, the center row has priority 1.0, and each row further away from the center has a priority of +1.0.
(e.g. row 5 will get updated 5 times more frequently than row 1.0)

You can find more information in the [book](https://github.com/cBournhonesque/lightyear/blob/main/book/src/concepts/advanced_replication/bandwidth_management.md)


https://github.com/cBournhonesque/lightyear/assets/8112632/41a6d102-77a1-4a44-8974-1d208b4ef798

## Running the example

To start the server, run `cargo run -- server`

Then you can launch multiple clients with the commands:

- `cargo run -- client -c 1`
- `cargo run -- client -c 2 --client-port 2000`




