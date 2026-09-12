# Timeline Switch (carry demo)

Server-authoritative physics demo for client-local prediction switching
(`TimelineSwitch`): inert blocks replicate as **interpolated** for everyone,
and each client switches nearby blocks to **predicted** — whatever carries them.

## Rules

- Walk into a free block to grab it (touch pickup). Press `E` to drop it.
- Cubes stay frozen on your head while carried; spheres dangle from the same
  spot on a leash, swinging with the same bounce on every screen.
- A block you carry is **predicted** on your client (green) so contacts use
  your timeline.
- A block carried by someone else is **predicted** (orange) while near you —
  held exactly at the carrier's carry pose with no interpolation lag — and
  **interpolated** once far, where the delay is invisible.
- Free blocks are predicted while near you (magenta) and interpolated when
  far (blue).
- Players work the same way: your character is always predicted, and nearby
  players are predicted too so shoves and collisions resolve on one timeline.
  Far players interpolate.

## Controls

`W/A/S/D` move, `Space` jump, `E` drop.

## Running it

- Server (headless): `cargo run -p timeline_switch -- --headless=true server`
- Client 1: `cargo run -p timeline_switch -- client -c 1`
- Client 2: `cargo run -p timeline_switch -- client -c 2`

Headless scripted run (used to verify the switching end to end):

```sh
cargo run -p timeline_switch -- --headless=true server &
LIGHTYEAR_AUTOMOVE="W*6,E,NONE*6" cargo run -p timeline_switch -- --headless=true client -c 1 &
LIGHTYEAR_AUTOMOVE="W*5,NONE*7" cargo run -p timeline_switch -- --headless=true client -c 2 &
wait
```

`LIGHTYEAR_AUTOMOVE` is a comma-separated `key[*secs]` script (`W`, `A`,
`S`, `D`, `SPACE`, `E`, `NONE`).

What to look for in the logs:

- server: `picked up block` / `dropping block`;
- client 1: `switching block to predicted (nearby)`, then no switch while
  carrying (already predicted);
- client 2: `switching block to predicted (nearby)` for free blocks, and
  `switching block to predicted (carried by another player)` for the carried
  block while close — it only switches to `... interpolated (carried by another
  player)` once far away.

Note: timeline switching is client-local and disabled in host-client mode.

## Inspecting what a switch does over time

The example emits the structured Lightyear debug stream, which is the supported
way to look at how a switch behaves frame by frame: one `character_last` row per
character per rendered frame, with the rendered `transform`, the simulated
`position`, the `correction` still to be given up, and which timeline the entity
is on.

```sh
LIGHTYEAR_DEBUG_FILE=debug.jsonl RUST_LOG="info,lightyear_debug=trace" \
  cargo run -p timeline_switch -- --headless=true client -c 1
```

Then query it with duckdb — for example, the rendered jump per frame, which is
what a visible discontinuity is:

```sh
duckdb -c "select frame_id, entity, transform, correction from \
  read_json_auto('debug.jsonl') where kind='character_last' order by frame_id"
```

`visual_correction_created` adds the jump a correction was measured from
(`previous`, `current_visual`, `error`), and `previous_visual_stored` /
`prepare_rollback_component` show where the rollback captured it. A switch is the
frame where a correction appears with a fresh `start_secs`; see the analysis in
the PR notes for how to read the result.

To reproduce the timeline sync and switch behaviour under load, add a client:

```sh
LIGHTYEAR_AUTOMOVE="NONE*4,S*5,NONE*2,W*6,NONE*9" \
  cargo run -p timeline_switch -- --headless=true client -c 2
```

walks that client away from client 1 (out of the switch radius) and back.
