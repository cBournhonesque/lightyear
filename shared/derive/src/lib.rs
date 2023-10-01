//! # Lightyear Derive
//! Procedural macros to simplify implementation of Lightyear protocol types

use quote::quote;

use channel::channel_impl;
use message::message_impl;

mod channel;

mod message;
mod shared;

// Channel

/// Derives the Channel trait for a given struct
#[proc_macro_derive(Channel)]
pub fn channel_derive(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let shared_crate_name = quote! { lightyear_shared };
    channel_impl(input, shared_crate_name)
}

#[proc_macro_derive(ChannelInternal)]
pub fn channel_derive_internal(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let shared_crate_name = quote! { crate };
    channel_impl(input, shared_crate_name)
}

// Message

/// Derives the Message trait for a given struct, for internal
#[proc_macro_derive(MessageInternal)]
pub fn message_derive_internal(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let shared_crate_name = quote! { crate };
    message_impl(input, shared_crate_name)
}

/// Derives the Message trait for a given struct
#[proc_macro_derive(Message)]
pub fn message_derive_shared(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let shared_crate_name = quote! { lightyear_shared };
    message_impl(input, shared_crate_name)
}
