use proc_macro2::{Ident, TokenStream};
use quote::quote;
use syn::DataStruct;

#[allow(clippy::format_push_string)]
pub fn derive_serde_tuple_struct(struct_: &DataStruct, struct_name: &Ident) -> TokenStream {
    let mut ser_body = quote! {};
    let mut de_body = quote! {};

    for (i, _) in struct_.fields.iter().enumerate() {
        let field_index = i;
        ser_body = quote! {
            #ser_body
            self.#field_index.ser(writer);
        };
        de_body = quote! {
            #de_body
            #field_index: Serde::de(reader)?,
        };
    }
    quote! {
        impl Serde for #struct_name {
             fn ser(&self, writer: &mut dyn lightyear_serde::BitWrite) {
                #ser_body
             }
             fn de(reader: &mut lightyear_serde::BitReader) -> std::result::Result<Self, lightyear_serde::SerdeErr> {
                std::result::Result::Ok(Self {
                    #de_body
                })
             }

        }
    }
}
