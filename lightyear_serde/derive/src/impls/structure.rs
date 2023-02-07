use proc_macro2::{Ident, TokenStream};
use quote::quote;
use syn::DataStruct;

#[allow(clippy::format_push_string)]
pub fn derive_serde_struct(struct_: &DataStruct, struct_name: &Ident) -> TokenStream {
    let mut ser_body = quote! {};
    let mut de_body = quote! {};

    for field in &struct_.fields {
        let field_name = field.ident.as_ref().expect("expected field to have a name");
        ser_body = quote! {
            #ser_body
            self.#field_name.ser(writer);
        };
        de_body = quote! {
            #de_body
            #field_name: Serde::de(reader)?,
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
