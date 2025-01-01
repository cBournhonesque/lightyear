# Changelog


## [Unreleased]

- Removed `DisabledComponent::<C>` in favor of `DisabledComponents` to have more control over
which components are disabled. In particular, it is now possible to express 'disable all components except these'.
- Enabled replicating events directly!
  - Add an `Event` to the protocol with `register_event`
  - Replicate an event and buffer it in EventWriter with `send_event`
  - Replicate an event and trigger it with `trigger_event`


## 0.18.0 - 2024-12-24