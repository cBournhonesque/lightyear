//! Replication visibility and interest management.
//!
//! Lightyear provides entity-level helpers in [`immediate`] and room-based
//! interest management in [`room`]. For component-level visibility, register a
//! Replicon [`VisibilityFilter`] whose scope contains the components to gate.
//! Add the filter component to the replicated entity and its
//! [`VisibilityFilter::ClientComponent`] to each sender link entity. If both
//! sides use the same component type, set `ClientComponent = Self`:
//!
//! ```rust,ignore
//! use bevy_ecs::prelude::*;
//! use bevy_replicon::prelude::{
//!     AppVisibilityExt, SingleComponent, VisibilityFilter,
//! };
//!
//! #[derive(Component, PartialEq)]
//! #[component(immutable)]
//! struct Team(u8);
//!
//! #[derive(Component)]
//! struct PrivateState(u32);
//!
//! impl VisibilityFilter for Team {
//!     type ClientComponent = Self;
//!     type Scope = SingleComponent<PrivateState>;
//!
//!     fn is_visible(
//!         &self,
//!         _client: Entity,
//!         client_team: Option<&Self::ClientComponent>,
//!     ) -> bool {
//!         client_team == Some(self)
//!     }
//! }
//!
//! // Register the filter and its component scope during app setup.
//! app.add_visibility_filter::<Team>();
//!
//! // `sender_link` is the entity representing this client connection.
//! commands.entity(sender_link).insert(Team(1));
//! commands.spawn((Replicate::default(), PrivateState(42), Team(1)));
//! ```
//!
//! Use a tuple such as `(Health, Inventory)` for `Scope` when several
//! components should share one visibility bit. The filter and client component
//! can also be different types when client permissions and entity metadata are
//! represented separately.
//!
//! [`VisibilityFilter`]: bevy_replicon::prelude::VisibilityFilter
//! [`VisibilityFilter::ClientComponent`]: bevy_replicon::prelude::VisibilityFilter::ClientComponent

pub mod immediate;

pub mod error;
pub mod room;
