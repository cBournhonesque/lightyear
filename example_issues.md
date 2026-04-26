# Avian physics

Seems to work (including host-client), apart from some weird artifacts at the beginning. Maybe the first rollback is weird?
Is the timeline sync ok? or we have too much prediction history?

# Bevy enhanced inputs

Client-server: inputs don't work.
Host-client: also seems broken.

# Deterministic replication

Debug later.

# FPS

Client-server: The movement seems to go too fast, maybe the movement system runs on both client and server?
(i compiled the binary with both client and server features enabled)

Prespawned bullets is broken: i see duplicates or errors.

# Lobby

For a lobby where the server is hosting: inputs were broken for one of the players.

For a lobby where one of the players is hosting: the inputs don't work for the host.

# Network visibility

Host-client: inputs don't work on the host.

# Priority

TODO: (not now, later) port to replicon's priority handling

# Projectiles

On the client; moving the cursor works but pressing WASD doesn't work.
Also the other keyboard inputs (Q, etc.) don't work.

# Replication groups

Host-client: the host doesn't seem to be able to move their entity

# Simple box

Client-server: still a bit broken (both prediction and interpolation) at the beginning.
Host-client: the host doesn't seem to be able to move their entity

# Spaceships
Host-client: i see some disappearing projectiles from both the host and the client. Maybe a prespawned issue?
The walls are jittering during rollbacks

Also the first rollback seems fairly buggy; maybe we are missing some cleanup and the initial rollback is too big?

