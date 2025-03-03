//! # Lightyear Derive
//! Procedural macros to simplify implementation of Lightyear protocol types
// TODO: remove this
#![allow(dead_code)]
#![allow(unused)]

use channel::channel_impl;
use proc_macro2::{Ident, Span};
use quote::{format_ident, quote};
use syn::{parse_macro_input, ItemEnum};

mod channel;
mod shared;

// Channel
#[doc(hidden)]
#[proc_macro_derive(ChannelInternal)]
pub fn channel_derive_internal(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let shared_crate_name = quote! { crate };
    channel_impl(input, shared_crate_name)
}

/// Derives the Channel trait for a given struct
#[proc_macro_derive(Channel)]
pub fn channel_derive(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let shared_crate_name = quote! { lightyear };
    channel_impl(input, shared_crate_name)
}
