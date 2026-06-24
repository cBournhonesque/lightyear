//! Pluggable validation for server-side input messages.
//!
//! Input messages arrive untrusted from remote clients. After they are
//! deserialized and *before* the server applies them to any
//! [`InputBuffer`](crate::input_buffer::InputBuffer), each message is run
//! through an ordered chain of [`InputMessageValidator`]s. A validator may
//!
//! - **mutate** the message — e.g. drop unwanted [`InputTarget`]s via
//!   [`InputMessage::inputs`]`.retain(..)`, or
//! - **reject** the whole message — returning [`InputValidation::Reject`],
//!   which skips it entirely (no buffer writes, no rebroadcast).
//!
//! This module provides *only* the extension point; lightyear ships no
//! validators by default, so the chain is empty (a no-op) until you register
//! one. A validator can be any type implementing [`InputMessageValidator`], or
//! simply a closure (see the blanket impl):
//!
//! ```ignore
//! app.add_input_validator::<MySequence>(|ctx, msg| {
//!     if msg.inputs.len() > 8 { InputValidation::Reject } else { InputValidation::Continue }
//! });
//! ```
//!
//! Register, group, replace, or remove validators through
//! [`InputValidatorAppExt`].

use alloc::boxed::Box;
use alloc::vec::Vec;
use bevy_app::App;
use bevy_ecs::entity::Entity;
use bevy_ecs::resource::Resource;
use lightyear_core::id::RemoteId;
use lightyear_core::tick::Tick;

use crate::input_message::{ActionStateSequence, InputMessage};

/// Read-only context handed to every [`InputMessageValidator`].
///
/// Validators run *inside* the input-receive system, so they cannot issue
/// their own ECS queries; everything they are expected to need is exposed
/// here. The set of fields is intentionally small and is expected to grow as
/// new validators need more context (see the module docs and the PR
/// discussion on scope).
pub struct InputValidationContext {
    /// The sender's connection entity on the server (the `ClientOf` / link
    /// entity the message was received on).
    pub sender: Entity,
    /// The sender's network id. `client_id.is_local()` is the host client.
    pub client_id: RemoteId,
    /// The server's current tick.
    pub server_tick: Tick,
}

/// The outcome of validating a single [`InputMessage`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputValidation {
    /// Keep the (possibly mutated) message and run the next validator.
    Continue,
    /// Drop the whole message: stop the chain and skip handling it.
    Reject,
}

/// A check applied to each received [`InputMessage`] before it is handled.
///
/// Implementations are registered per action-state-sequence type `S` via
/// [`InputValidatorAppExt`] and stored in an [`InputMessageValidators<S>`]
/// resource. They run in registration order; the first [`InputValidation::Reject`]
/// drops the message.
///
/// Any matching closure also implements this trait (see the blanket impl
/// below), so simple one-off checks need no dedicated type.
pub trait InputMessageValidator<S: ActionStateSequence>: Send + Sync + 'static {
    /// Inspect (and optionally mutate) `message`. Return
    /// [`InputValidation::Reject`] to drop it.
    fn validate(
        &self,
        ctx: &InputValidationContext,
        message: &mut InputMessage<S>,
    ) -> InputValidation;

    /// A stable identifier used to remove or replace this validator (see
    /// [`InputValidatorAppExt::remove_input_validator`]). Defaults to the
    /// concrete type name.
    fn name(&self) -> &'static str {
        core::any::type_name::<Self>()
    }
}

/// Any `Fn(&InputValidationContext, &mut InputMessage<S>) -> InputValidation`
/// is an [`InputMessageValidator`], so one-off checks can be registered as a
/// closure without defining a type.
///
/// Closures take the default [`name`](InputMessageValidator::name) (their
/// opaque type name), so they are not practical to remove by name — define a
/// named type if you need [`InputValidatorAppExt::remove_input_validator`].
impl<S, F> InputMessageValidator<S> for F
where
    S: ActionStateSequence,
    F: Fn(&InputValidationContext, &mut InputMessage<S>) -> InputValidation
        + Send
        + Sync
        + 'static,
{
    fn validate(
        &self,
        ctx: &InputValidationContext,
        message: &mut InputMessage<S>,
    ) -> InputValidation {
        self(ctx, message)
    }
}

/// Resource holding the ordered validator chain for [`InputMessage<S>`].
///
/// Inserted (empty) by `ServerInputPlugin`; mutate it through
/// [`InputValidatorAppExt`].
#[derive(Resource)]
pub struct InputMessageValidators<S: ActionStateSequence> {
    validators: Vec<Box<dyn InputMessageValidator<S>>>,
}

impl<S: ActionStateSequence> Default for InputMessageValidators<S> {
    fn default() -> Self {
        Self {
            validators: Vec::new(),
        }
    }
}

impl<S: ActionStateSequence> InputMessageValidators<S> {
    /// Append a single validator (a type or a closure).
    pub fn push(&mut self, validator: impl InputMessageValidator<S>) {
        self.validators.push(Box::new(validator));
    }

    /// Append several already-boxed validators (a group).
    pub fn extend(
        &mut self,
        validators: impl IntoIterator<Item = Box<dyn InputMessageValidator<S>>>,
    ) {
        self.validators.extend(validators);
    }

    /// Remove every validator.
    pub fn clear(&mut self) {
        self.validators.clear();
    }

    /// Remove every validator whose [`InputMessageValidator::name`] matches.
    pub fn remove(&mut self, name: &str) {
        self.validators.retain(|v| v.name() != name);
    }

    /// Number of registered validators.
    pub fn len(&self) -> usize {
        self.validators.len()
    }

    /// Whether the chain is empty.
    pub fn is_empty(&self) -> bool {
        self.validators.is_empty()
    }

    /// Run the chain over `message`. Returns `false` if any validator rejected
    /// it (the caller should then skip the message).
    pub fn validate(
        &self,
        ctx: &InputValidationContext,
        message: &mut InputMessage<S>,
    ) -> bool {
        for validator in &self.validators {
            if validator.validate(ctx, message) == InputValidation::Reject {
                return false;
            }
        }
        true
    }
}

/// App-builder extension for registering, grouping, and removing
/// [`InputMessageValidator`]s.
pub trait InputValidatorAppExt {
    /// Append one validator (type or closure) to the chain for `S`.
    fn add_input_validator<S: ActionStateSequence>(
        &mut self,
        validator: impl InputMessageValidator<S>,
    ) -> &mut Self;

    /// Append a group of already-boxed validators to the chain for `S`.
    fn add_input_validators<S: ActionStateSequence>(
        &mut self,
        validators: impl IntoIterator<Item = Box<dyn InputMessageValidator<S>>>,
    ) -> &mut Self;

    /// Remove all validators for `S`.
    fn clear_input_validators<S: ActionStateSequence>(&mut self) -> &mut Self;

    /// Remove the validator(s) named `name` for `S`.
    fn remove_input_validator<S: ActionStateSequence>(&mut self, name: &str) -> &mut Self;
}

impl InputValidatorAppExt for App {
    fn add_input_validator<S: ActionStateSequence>(
        &mut self,
        validator: impl InputMessageValidator<S>,
    ) -> &mut Self {
        self.world_mut()
            .get_resource_or_init::<InputMessageValidators<S>>()
            .push(validator);
        self
    }

    fn add_input_validators<S: ActionStateSequence>(
        &mut self,
        validators: impl IntoIterator<Item = Box<dyn InputMessageValidator<S>>>,
    ) -> &mut Self {
        self.world_mut()
            .get_resource_or_init::<InputMessageValidators<S>>()
            .extend(validators);
        self
    }

    fn clear_input_validators<S: ActionStateSequence>(&mut self) -> &mut Self {
        self.world_mut()
            .get_resource_or_init::<InputMessageValidators<S>>()
            .clear();
        self
    }

    fn remove_input_validator<S: ActionStateSequence>(&mut self, name: &str) -> &mut Self {
        self.world_mut()
            .get_resource_or_init::<InputMessageValidators<S>>()
            .remove(name);
        self
    }
}
