//! # Naia Derive
//! Procedural macros to simplify implementation of Naia Replicate &
//! Protocolize traits

#![deny(trivial_casts, trivial_numeric_casts, unstable_features)]

mod channel_index;
mod protocolize;
mod replicate;

use channel_index::channel_index_impl;
use protocolize::protocolize_impl;
use replicate::replicate_impl;

/// Derives the Protocolize trait for a given enum
#[proc_macro_derive(Protocolize)]
pub fn protocolize_derive(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    protocolize_impl(input)
}

/// Derives the Replicate trait for a given struct
#[proc_macro_derive(Replicate, attributes(protocol_path))]
pub fn replicate_derive(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    replicate_impl(input)
}

#[proc_macro_derive(ChannelIndex)]
pub fn channel_index_derive(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    channel_index_impl(input)
}
