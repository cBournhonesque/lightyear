# Time sync

Ticks are the shared clock. The server's tick is authoritative; the client continuously estimates it and steers its own timeline to match.

The pieces:
- `LocalTimeline`: a resource with the local simulation tick. It advances once per `FixedMain` run.
- `RemoteTimeline`: a component on the link entity holding the estimated tick of the remote peer, built from packet header ticks plus ping measurements.
- `LocalTimelineSync`: the controller. It compares the local instant against the remote estimate and either shifts the tick by whole ticks or speeds/slows the simulation. It also owns the input delay: the number of ticks the client waits before applying its own inputs, so they still arrive at the server on time.
- `SyncedLocalTimeline`: a system param for gameplay systems that must not run before sync is ready. It derefs to `LocalTimeline` and also exposes the input delay.
   Systems holding this SystemParam will be skipped while the local timeline is not synced to the remote.
- `SyncedInterpolationTimeline`: same idea for systems that need the interpolation cursor (the slightly-in-the-past sampling point) to be ready.
- `LocalTimelineShift`: an event emitted on whole-tick corrections, so input buffers, prediction history and prespawn state all shift together.

Ping (smoothed RTT with outlier rejection) feeds all of this. You mostly don't touch these types directly; but when a system behaves oddly at startup (entities frozen for the first second), "timeline not synced yet" is the usual cause, and gating that system on `SyncedLocalTimeline` is the usual fix.
