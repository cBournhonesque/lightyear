//! Module to handle inputs that are defined using the `leafwing_input_manager` crate
//!
//! ### Adding leafwing inputs
//!
//! You first need to create Inputs that are defined using the [`leafwing_input_manager`](https://github.com/Leafwing-Studios/leafwing-input-manager) crate.
//! (see the documentation of the crate for more information)
//! In particular your inputs should implement the [`Actionlike`] trait.
//!
//! ```rust
//! # use bevy_app::App;
//! # use bevy_reflect::Reflect; 
//! # use serde::{Deserialize, Serialize};
//! use leafwing_input_manager::Actionlike;
//! use lightyear_inputs_leafwing::prelude::InputPlugin;
//!
//! #[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Copy, Hash, Reflect, Actionlike)]
//! pub enum PlayerActions {
//!     Up,
//!     Down,
//!     Left,
//!     Right,
//! }
//!
//! let mut app = App::new();
//! app.add_plugins(InputPlugin::<PlayerActions>::default());
//! ```
//!
//! ### Usage
//!
//! The networking of inputs is completely handled for you. You just need to add the `InputPlugin` to your app.
//! Make sure that all your systems that depend on user inputs are added to the [`FixedUpdate`] [`Schedule`].
//!
//! Currently, global inputs (that are stored in a [`Resource`] instead of being attached to a specific [`Entity`] are not supported)
#![no_std]

extern crate alloc;
extern crate core;
#[cfg(feature = "std")]
extern crate std;

pub(crate) mod action_diff;

mod action_state;

mod input_message;

mod plugin;

pub mod prelude {
    pub use crate::plugin::InputPlugin;
}
