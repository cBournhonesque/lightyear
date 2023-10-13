//! # Lightyear Derive
//! Procedural macros to simplify implementation of Lightyear protocol types
// TODO: remove this
#![allow(dead_code)]
#![allow(unused)]

use proc_macro2::{Ident, Span};
use quote::{format_ident, quote};
use syn::{parse_macro_input, ItemEnum};

use channel::channel_impl;
use component::component_impl;
use message::message_impl;

mod channel;
mod component;
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
#[proc_macro_derive(MessageProtocol)]
pub fn message_derive_internal(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    message_impl(input)
}

#[proc_macro_attribute]
pub fn message_protocol_internal(
    _metadata: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let item = parse_macro_input!(input as ItemEnum);
    let enum_name = &item.ident;
    let lowercase_struct_name = Ident::new(
        enum_name.to_string().to_lowercase().as_str(),
        Span::call_site(),
    );
    let module_name = format_ident!("define_{}", lowercase_struct_name);

    let output = quote! {
        mod #module_name {
            use super::*;
            use serde::{Serialize, Deserialize};
            use crate::{ReadBuffer, WriteBuffer, BitSerializable, MessageProtocol};

            #[derive(MessageProtocol, Serialize, Deserialize, Clone)]
            #item
        }
        pub use #module_name::#enum_name as #enum_name;

    };
    output.into()
}

#[proc_macro_attribute]
pub fn message_protocol(
    _metadata: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let item = parse_macro_input!(input as ItemEnum);
    let enum_name = &item.ident;
    let lowercase_struct_name = Ident::new(
        enum_name.to_string().to_lowercase().as_str(),
        Span::call_site(),
    );
    let module_name = format_ident!("define_{}", lowercase_struct_name);

    let output = quote! {
        mod #module_name {
            use super::*;
            use serde::{Serialize, Deserialize};
            use lightyear_shared::{ReadBuffer, WriteBuffer, BitSerializable, MessageProtocol};

            #[derive(MessageProtocol, Serialize, Deserialize, Clone)]
            #item
        }
        pub use #module_name::#enum_name as #enum_name;

    };
    output.into()
}

// Components

/// Derives the component trait for a given struct, for internal
#[proc_macro_derive(ComponentProtocol)]
pub fn component_derive_internal(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    component_impl(input)
}

#[proc_macro_attribute]
pub fn component_protocol_internal(
    _metadata: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let item = parse_macro_input!(input as ItemEnum);
    let enum_name = &item.ident;
    let enum_kind_name = format_ident!("{}Kind", enum_name);
    let lowercase_struct_name = Ident::new(
        enum_name.to_string().to_lowercase().as_str(),
        Span::call_site(),
    );
    let module_name = format_ident!("define_{}", lowercase_struct_name);

    let output = quote! {
        mod #module_name {
            use super::*;
            use serde::{Serialize, Deserialize};
            use crate::{EnumKind, ReadBuffer, WriteBuffer, BitSerializable, ComponentProtocol};

            #[derive(ComponentProtocol, EnumKind, Serialize, Deserialize, Clone)]
            #[enum_kind(#enum_kind_name, derive(Serialize, Deserialize))]
            #item
        }
        pub use #module_name::#enum_name as #enum_name;
        pub use #module_name::#enum_kind_name as #enum_kind_name;
    };
    output.into()
}

#[proc_macro_attribute]
pub fn component_protocol(
    _metadata: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let item = parse_macro_input!(input as ItemEnum);
    let enum_name = &item.ident;
    let enum_kind_name = format_ident!("{}Kind", enum_name);
    let lowercase_struct_name = Ident::new(
        enum_name.to_string().to_lowercase().as_str(),
        Span::call_site(),
    );
    let module_name = format_ident!("define_{}", lowercase_struct_name);

    let output = quote! {
        mod #module_name {
            use super::*;
            use serde::{Serialize, Deserialize};
            use lightyear_shared::{EnumKind, ReadBuffer, WriteBuffer, BitSerializable, ComponentProtocol};

            #[derive(ComponentProtocol, EnumKind, Serialize, Deserialize, Clone)]
            #[enum_kind(#enum_kind_name, derive(Serialize, Deserialize))]
            #item
        }
        pub use #module_name::#enum_name as #enum_name;
        pub use #module_name::#enum_kind_name as #enum_kind_name;

    };
    output.into()
}

fn get_module_name_for_enum(item: &ItemEnum) -> Ident {
    let enum_name = &item.ident;
    let lowercase_struct_name = Ident::new(
        enum_name.to_string().to_lowercase().as_str(),
        Span::call_site(),
    );
    format_ident!("define_{}", lowercase_struct_name)
}
