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
    let struct_name = &input.ident;
    let name = syn::LitStr::new(&struct_name.to_string(), Span::call_site());
    let (impl_generics, type_generics, where_clause) = &input.generics.split_for_impl();

    let tokens = quote! {
        impl #impl_generics #shared_crate_name::prelude::Channel for #struct_name #type_generics #where_clause {
            fn name() -> &'static str {
                #name
            }
        }
    };

    proc_macro::TokenStream::from(tokens)
}
