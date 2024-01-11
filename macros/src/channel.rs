use proc_macro2::{Ident, Span, TokenStream};
use quote::{format_ident, quote};
use syn::{parse_macro_input, DeriveInput, LitStr};

use super::shared::{get_struct_type, StructType};

pub fn channel_impl(
    input: proc_macro::TokenStream,
    shared_crate_name: TokenStream,
) -> proc_macro::TokenStream {
    let input = parse_macro_input!(input as DeriveInput);

    // Helper Properties
    let struct_type = get_struct_type(&input);
    match struct_type {
        StructType::Struct | StructType::TupleStruct => {
            panic!("Can only derive Channel on a Unit struct (i.e. `struct MyStruct;`)");
        }
        _ => {}
    }

    // Names
    let struct_name = input.ident;
    let struct_name_str = LitStr::new(&struct_name.to_string(), struct_name.span());
    let lowercase_struct_name = Ident::new(
        struct_name.to_string().to_lowercase().as_str(),
        Span::call_site(),
    );
    let module_name = format_ident!("define_{}", lowercase_struct_name);

    // Methods
    let get_builder_method = get_builder_method();

    let gen = quote! {
        #[doc(hidden)]
        mod #module_name {
            pub use #shared_crate_name::prelude::{Channel, ChannelKind, ChannelBuilder, ChannelContainer, ChannelSettings, Named};
            use super::*;

            impl Channel for #struct_name {
                #get_builder_method
            }
            impl Default for #struct_name {
                fn default() -> Self {
                    Self
                }
            }
            impl Clone for #struct_name {
                fn clone(&self) -> Self {
                    Self
                }
            }
            impl Named for #struct_name {
                const NAME: &'static str = #struct_name_str;
            }
        }
    };

    proc_macro::TokenStream::from(gen)
}

fn get_builder_method() -> TokenStream {
    quote! {
        fn get_builder(settings: ChannelSettings) -> ChannelBuilder {
            ChannelBuilder{settings}
        }
    }
}
