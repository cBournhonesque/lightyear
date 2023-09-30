use quote::quote;
use syn::{parse_macro_input, Data, DeriveInput, Fields};

mod impls;
use impls::*;

#[proc_macro_derive(Serde)]
pub fn derive_serde(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let serde_crate_name = quote! { naia_shared };
    derive_serde_common(input, serde_crate_name)
}

#[proc_macro_derive(SerdeInternal)]
pub fn derive_serde_internal(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let serde_crate_name = quote! { naia_serde };
    derive_serde_common(input, serde_crate_name)
}

#[proc_macro_derive(SerdeBevy)]
pub fn derive_serde_bevy(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let serde_crate_name = quote! { naia_bevy_shared };
    derive_serde_common(input, serde_crate_name)
}

#[proc_macro_derive(SerdeHecs)]
pub fn derive_serde_hecs(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let serde_crate_name = quote! { naia_hecs_shared };
    derive_serde_common(input, serde_crate_name)
}

fn derive_serde_common(
    input: proc_macro::TokenStream,
    serde_crate_name: proc_macro2::TokenStream,
) -> proc_macro::TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let input_name = input.ident;

    let gen = match &input.data {
        Data::Enum(enum_) => derive_serde_enum(enum_, &input_name, serde_crate_name),
        Data::Struct(struct_) => match struct_.fields {
            Fields::Unit | Fields::Unnamed(_) => {
                derive_serde_tuple_struct(struct_, &input_name, serde_crate_name)
            }
            Fields::Named(_) => derive_serde_struct(struct_, &input_name, serde_crate_name),
        },
        _ => unimplemented!("Only structs and enums are supported"),
    };

    proc_macro::TokenStream::from(gen)
}
