//! # Lightyear Derive
//! Procedural macros to simplify implementation of Lightyear protocol types

mod channel;
mod shared;

use channel::channel_impl;
use quote::quote;

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
