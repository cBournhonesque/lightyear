//! # Lightyear Derive
//! Procedural macros to simplify implementation of Lightyear protocol types
// TODO: remove this
#![allow(dead_code)]
#![allow(unused)]

use proc_macro2::{Ident, Span};
use quote::{format_ident, quote};
use syn::{parse_macro_input, ItemEnum};

use channel::channel_impl;
use component::component_protocol_impl;
use message::{message_impl, message_protocol_impl};

mod channel;
mod component;
mod message;
mod shared;

// Channel

/// Derives the Channel trait for a given struct
#[proc_macro_derive(Channel)]
pub fn channel_derive(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let shared_crate_name = quote! { lightyear };
    channel_impl(input, shared_crate_name)
}

#[doc(hidden)]
#[proc_macro_derive(ChannelInternal)]
pub fn channel_derive_internal(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let shared_crate_name = quote! { crate };
    channel_impl(input, shared_crate_name)
}

// Message

/// Derives the Message trait for a given struct
#[proc_macro_derive(Message, attributes(message))]
pub fn message_derive(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let shared_crate_name = quote! { lightyear };
    message_impl(input, shared_crate_name)
}

#[doc(hidden)]
#[proc_macro_derive(MessageInternal, attributes(message))]
pub fn message_derive_internal(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let shared_crate_name = quote! { crate };
    message_impl(input, shared_crate_name)
}

#[doc(hidden)]
#[proc_macro_attribute]
pub fn message_protocol_internal(
    args: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let shared_crate_name = quote! { crate };
    message_protocol_impl(args, input, shared_crate_name)
}

/// Attribute macro applied to an enum to derive the MessageProtocol trait for it
#[proc_macro_attribute]
pub fn message_protocol(
    args: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let shared_crate_name = quote! { lightyear };
    message_protocol_impl(args, input, shared_crate_name)
}

// Components

#[doc(hidden)]
#[proc_macro_attribute]
pub fn component_protocol_internal(
    args: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let shared_crate_name = quote! { crate };
    component_protocol_impl(args, input, shared_crate_name)
}

/// Attribute macro applied to an enum to derive the ComponentProtocol trait for it
#[proc_macro_attribute]
pub fn component_protocol(
    args: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let shared_crate_name = quote! { lightyear };
    component_protocol_impl(args, input, shared_crate_name)
}
