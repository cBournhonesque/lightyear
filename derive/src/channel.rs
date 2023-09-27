use proc_macro2::{Ident, Span, TokenStream};
use quote::{format_ident, quote};
use syn::{parse_macro_input, DeriveInput};

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
    let lowercase_struct_name = Ident::new(
        struct_name.to_string().to_lowercase().as_str(),
        Span::call_site(),
    );
    let module_name = format_ident!("define_{}", lowercase_struct_name);
    let builder_name = format_ident!("{}Builder", struct_name);

    // Methods
    let build_method = get_build_method();
    let get_builder_method = get_builder_method(&builder_name);

    let gen = quote! {
        mod #module_name {
            pub use #shared_crate_name::{Channel, ChannelBuilder, ChannelContainer, ChannelSettings};
            use super::*;

            pub struct #builder_name{
                settings: ChannelSettings
            }
            impl ChannelBuilder for #builder_name {
                #build_method
            }
            impl Channel for #struct_name {
                #get_builder_method
            }
        }
    };

    proc_macro::TokenStream::from(gen)
}

fn get_build_method() -> TokenStream {
    quote! {
        fn build(&self) -> ChannelContainer {
            // TODO: use Option instead to avoid having to derive Default on settings
            ChannelContainer::new(std::mem::take(&mut self.settings))
        }
    }
}

fn get_builder_method(builder_name: &Ident) -> TokenStream {
    quote! {
        fn get_builder(settings: ChannelSettings) -> Box<dyn ChannelBuilder> {
            Box::new(#builder_name{settings})
        }
    }
}
