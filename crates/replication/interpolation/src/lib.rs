//! Interpolation for replicated entities.
//!
//! Network replication delivers state at discrete server ticks. Rendering those
//! updates directly can make remote entities snap from one received value to the
//! next. This crate keeps a delayed history of received component values and
//! applies interpolated values on entities with [`Interpolated`].
//!
//! # Basic setup
//!
//! Add [`plugin::InterpolationPlugin`] on the client and register interpolation rules in
//! the same shared protocol code that registers replicated components. The
//! rules are stored in [`registry::InterpolationRegistry`].
//!
//! A full interpolation rule owns the whole delayed-interpolation pipeline for a
//! component:
//!
//! - received values are stored in
//!   [`ConfirmedHistory<C>`](lightyear_core::prelude::ConfirmedHistory),
//! - the history is sampled at the client's [`timeline::InterpolationTimeline`],
//! - and the sampled value is written back to the live component.
//!
//! Applying a sampled value uses Bevy's normal component change detection, so
//! systems ordered after interpolation can observe it with `Changed<C>`.
//!
//! ```rust,ignore
//! use bevy_app::App;
//! use bevy_ecs::prelude::*;
//! use lightyear_interpolation::prelude::*;
//! use lightyear_replication::prelude::*;
//! use serde::{Deserialize, Serialize};
//!
//! #[derive(Component, Clone, PartialEq, Serialize, Deserialize)]
//! struct Position(f32);
//!
//! fn lerp_position(start: Position, end: Position, t: f32) -> Position {
//!     Position(start.0 + (end.0 - start.0) * t)
//! }
//!
//! fn protocol(app: &mut App) {
//!     app.component::<Position>().replicate();
//!     app.interpolate_with::<Position>(InterpolationFns::interpolate(lerp_position));
//! }
//! ```
//!
//! If `C` implements [`Ease`](bevy_math::curve::Ease), the common linear case can
//! be registered with [`registry::AppInterpolationExt::linear_interpolate`]:
//!
//! ```rust,ignore
//! # use bevy_app::App;
//! # use bevy_ecs::prelude::*;
//! # use bevy_math::{Curve, curve::Ease};
//! # use lightyear_interpolation::prelude::*;
//! # use lightyear_replication::prelude::*;
//! # use serde::{Deserialize, Serialize};
//! #[derive(Component, Clone, PartialEq, Serialize, Deserialize)]
//! struct Position(f32);
//!
//! impl Ease for Position {
//!     fn interpolating_curve_unbounded(start: Self, end: Self) -> impl Curve<Self> {
//!         bevy_math::curve::FunctionCurve::new(bevy_math::curve::Interval::UNIT, move |t| {
//!             Position(start.0 + (end.0 - start.0) * t)
//!         })
//!     }
//! }
//!
//! fn protocol(app: &mut App) {
//!     app.component::<Position>().replicate();
//!     app.linear_interpolate::<Position>();
//! }
//! ```
//!
//! # Interpolation functions
//!
//! [`rules::InterpolationFns`] describes which work Lightyear owns for a rule:
//!
//! - [`rules::InterpolationFns::interpolate`] owns delayed history and applies the live
//!   component.
//! - [`rules::InterpolationFns::history_only`] owns delayed history but does not apply
//!   the live component. This is the usual choice when a custom system samples
//!   one or more histories and writes visuals itself.
//! - [`rules::InterpolationFns::history_only`] plus
//!   [`rules::InterpolationFnsExt::interpolate`] keeps an interpolation function for
//!   frame interpolation and correction, while still leaving delayed
//!   interpolation application to user code.
//! - [`rules::InterpolationFns::no_history`] stores only an interpolation function. It
//!   is useful for predicted entities that need frame interpolation or visual
//!   correction but do not receive delayed interpolation history.
//! - [`rules::InterpolationFns::disabled`] is a high-priority opt-out rule. If it is
//!   selected, lower-priority matching rules for the same component or bundle do
//!   not run.
//!
//! Components used with these rules must implement [`SyncComponent`]:
//! `Component<Mutability = Mutable> + Clone + PartialEq`.
//!
//! Most interpolation functions use the simple `fn(start, end, fraction)`
//! shape. Functions that need sample timing can be registered with
//! [`rules::InterpolationFns::interpolate_with_context`]; they receive
//! [`rules::InterpolationSampleContext`], which includes the normalized fraction
//! plus sample duration when it is available. This is useful for interpolation
//! methods such as Hermite curves that need to scale velocities by the interval
//! between samples.
//!
//! # Custom interpolation systems
//!
//! Use a history-only rule when Lightyear should still receive and maintain the
//! delayed history, but your own system decides how to apply it. This is useful
//! when interpolation depends on several components, render-only state, or game
//! specific constraints.
//!
//! ```rust,ignore
//! use bevy_app::{App, Update};
//! use bevy_ecs::prelude::*;
//! use lightyear_core::prelude::ConfirmedHistory;
//! use lightyear_interpolation::prelude::*;
//! use lightyear_replication::prelude::*;
//! use serde::{Deserialize, Serialize};
//!
//! #[derive(Component, Clone, PartialEq, Serialize, Deserialize)]
//! struct Position(f32);
//!
//! #[derive(Component)]
//! struct VisualPosition(f32);
//!
//! fn protocol(app: &mut App) {
//!     app.component::<Position>().replicate();
//!     app.interpolate_with::<Position>(InterpolationFns::history_only());
//!     app.add_systems(Update, custom_interpolation.in_set(InterpolationSystems::Interpolate));
//! }
//!
//! fn custom_interpolation(
//!     mut query: Query<(&ConfirmedHistory<Position>, &mut VisualPosition), With<Interpolated>>,
//! ) {
//!     for (history, mut visual) in &mut query {
//!         if let Some((_tick, position)) = history.get_nth_present(0) {
//!             visual.0 = position.0;
//!         }
//!     }
//! }
//! ```
//!
//! The example intentionally keeps the sampling simple. Real systems usually use
//! [`timeline::InterpolationTimeline`] and [`interpolate::interpolation_fraction`] to
//! choose the bracketing samples and interpolation fraction.
//!
//! # Rules, filters, and priority
//!
//! A rule is registered for a rule kind: either a single component `C` or a
//! tuple bundle such as `(Position, Rotation)`. Rules can also include an
//! [`ArchetypeFilter`](bevy_ecs::query::ArchetypeFilter), such as
//! `With<MyMarker>` or `Without<MyMarker>`.
//!
//! For each interpolated archetype, Lightyear considers every matching rule in
//! this order:
//!
//! 1. higher priority first,
//! 2. then earlier registration order for equal priority.
//!
//! Rule resolution assigns history maintenance and interpolation application
//! separately. Within each assignment, a selected rule atomically claims all
//! of its component members. A lower-priority component or bundle rule is
//! skipped if any of those members were already claimed for the same kind of
//! work.
//!
//! The default priority is the number of components in the interpolation target.
//! A default `(Position, Rotation)` bundle therefore takes priority over default
//! single-component `Position` and `Rotation` rules when all match the same
//! archetype.
//!
//! ## History ownership and apply ownership
//!
//! A registered rule describes both how its members' histories are maintained
//! and how those members are interpolated. Resolution can assign those two
//! responsibilities to different matching rules, however. This is necessary
//! because delayed component insertion and removal intentionally allow a
//! history component to exist while its corresponding live component does not.
//!
//! Resolution uses two independent kinds of ownership:
//!
//! | Ownership | A rule is eligible when... |
//! | --- | --- |
//! | History | Every member has either its live component or the relevant history component. |
//! | Apply | Every member has its live component. |
//!
//! Rules are considered in the priority and registration order described
//! above. History and apply each have their own set of claimed members, so one
//! rule can own history, apply, or both. Claiming a member for history does not
//! prevent another rule from claiming it for apply, and vice versa.
//! "History ownership" includes the decision not to keep history: when a
//! [`rules::InterpolationFns::no_history`] rule wins that lane, it has no history
//! callbacks to run and prevents a lower-priority rule from creating history
//! for the same members.
//!
//! For example, suppose a higher-priority bundle rule targets `(A, B)` and a
//! lower-priority component rule targets `A`:
//!
//! - During a delayed insertion, an entity can have live `A` plus histories for
//!   both `A` and `B`, while live `B` is waiting for its insertion tick. The
//!   bundle owns history maintenance, while the component rule applies `A`.
//!   When history maintenance inserts `B`, the bundle becomes eligible to apply
//!   both components.
//! - During a delayed removal, the bundle owns both responsibilities while `A`
//!   and `B` are live. Once history maintenance reaches the removal tick and
//!   removes `B`, the bundle continues to own both histories, while the
//!   component rule takes over applying the remaining live `A`.
//!
//! Missing history is different from a missing live component. If both `A` and
//! `B` are live but one history for a history-based bundle has not been created
//! yet, the bundle is still the apply winner. It creates and fills the missing
//! history, and apply waits until all required samples are queryable; a
//! lower-priority `A` rule does not temporarily take over merely because the
//! bundle cannot sample yet.
//!
//! ```rust,ignore
//! # use bevy_app::App;
//! # use bevy_ecs::prelude::*;
//! # use lightyear_interpolation::prelude::*;
//! # use lightyear_replication::prelude::*;
//! # use serde::{Deserialize, Serialize};
//! #[derive(Component, Clone, PartialEq, Serialize, Deserialize)]
//! struct Position(f32);
//!
//! #[derive(Component)]
//! struct SnapPosition;
//!
//! fn lerp_position(start: Position, end: Position, t: f32) -> Position {
//!     Position(start.0 + (end.0 - start.0) * t)
//! }
//!
//! fn protocol(app: &mut App) {
//!     app.component::<Position>().replicate();
//!     app.interpolate_with::<Position>(InterpolationFns::interpolate(lerp_position));
//!
//!     // Entities with `SnapPosition` match both rules. This higher-priority
//!     // disabled rule blocks interpolation for those archetypes.
//!     app.interpolate_with_priority_filtered::<Position, With<SnapPosition>>(
//!         100,
//!         InterpolationFns::disabled(),
//!     );
//! }
//! ```
//!
//! # Bundle interpolation
//!
//! Bundle rules let one interpolation function sample several component
//! histories together. Each component still has its own
//! [`ConfirmedHistory`](lightyear_core::prelude::ConfirmedHistory), but the
//! apply step fetches all member histories and calls the tuple interpolation
//! function.
//!
//! ```rust,ignore
//! # use bevy_app::App;
//! # use bevy_ecs::prelude::*;
//! # use lightyear_interpolation::prelude::*;
//! # use lightyear_replication::prelude::*;
//! # use serde::{Deserialize, Serialize};
//! #[derive(Component, Clone, PartialEq, Serialize, Deserialize)]
//! struct Position(f32);
//!
//! #[derive(Component, Clone, PartialEq, Serialize, Deserialize)]
//! struct Rotation(f32);
//!
//! fn interpolate_transform(
//!     start: (Position, Rotation),
//!     end: (Position, Rotation),
//!     t: f32,
//! ) -> (Position, Rotation) {
//!     (
//!         Position(start.0.0 + (end.0.0 - start.0.0) * t),
//!         Rotation(start.1.0 + (end.1.0 - start.1.0) * t),
//!     )
//! }
//!
//! fn protocol(app: &mut App) {
//!     app.component::<Position>().replicate();
//!     app.component::<Rotation>().replicate();
//!     app.interpolate_bundle_with::<(Position, Rotation)>(
//!         InterpolationFns::interpolate(interpolate_transform),
//!     );
//! }
//! ```
//!
//! Bundle interpolation is supported for tuple sizes up to 8.
//!
//! # Frame interpolation and correction
//!
//! The same interpolation rules are also reused by frame interpolation and
//! post-rollback visual correction when those plugins are enabled. In the common
//! case, register one interpolation rule in the protocol and add
//! [`FrameInterpolate`](lightyear_core::prelude::FrameInterpolate) to predicted
//! or visual entities that should be smoothed between fixed ticks.
//!
//! If an entity should use a rule only for delayed interpolation, or only for
//! frame interpolation, use filtered rules with marker components such as
//! [`Interpolated`] or
//! [`FrameInterpolate`](lightyear_core::prelude::FrameInterpolate).
//!
//! # Scheduling
//!
//! [`plugin::InterpolationPlugin`] runs the receive-history and apply systems in
//! [`plugin::InterpolationSystems`]. Custom interpolation systems should usually run in
//! [`plugin::InterpolationSystems::Interpolate`] so they see histories after Lightyear
//! has updated component presence for the interpolation timeline.
#![no_std]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

use bevy_ecs::component::{Component, Mutable};

#[doc(hidden)]
pub mod archetypes;
/// Handles delayed despawns for interpolated entities.
pub mod despawn;
/// Contains interpolation logic.
pub mod interpolate;
/// Provides the `InterpolationPlugin` and related systems for Bevy integration.
pub mod plugin;
pub mod registry;
/// Interpolation rule types and bundle support.
pub mod rules;
pub mod timeline;

/// Commonly used items for client-side interpolation.
pub mod prelude {
    pub use crate::Interpolated;
    pub use crate::interpolate::interpolation_fraction;
    pub use crate::plugin::{
        InterpolationDelay, InterpolationMarkerPlugin, InterpolationPlugin, InterpolationSystems,
    };
    pub use crate::registry::{
        AppInterpolationExt, InterpolationRegistrationExt, InterpolationRegistry,
    };
    pub use crate::rules::{
        ContextInterpolationFn, InterpolationBundle, InterpolationFn, InterpolationFns,
        InterpolationFnsExt, InterpolationRuleConfig, InterpolationSampleContext,
    };
    pub use crate::timeline::{
        InterpolationConfig, InterpolationTimeline, SyncedInterpolationTimeline,
    };
}

pub use lightyear_core::interpolation::Interpolated;

/// Trait for components that can be synchronized for interpolation.
///
/// This is a marker trait, requiring `Component<Mutability=Mutable> + Clone + PartialEq`.
/// Components implementing this trait can have their state managed by the interpolation systems
/// according to the specified `InterpolationMode`.
pub trait SyncComponent: Component<Mutability = Mutable> + Clone + PartialEq {}
impl<T> SyncComponent for T where T: Component<Mutability = Mutable> + Clone + PartialEq {}
