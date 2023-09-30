use proc_macro2::{Ident, Span, TokenStream};
use quote::{format_ident, quote};
use syn::{DataEnum, Fields};

fn bits_needed_for(max_value: usize) -> u8 {
    let mut bits = 1;
    while 2_usize.pow(bits) <= max_value {
        bits += 1;
    }
    if bits >= 256 {
        panic!("cannot encode a number in more than 255 bits!");
    }
    bits as u8
}

#[allow(clippy::format_push_string)]
pub fn derive_serde_enum(
    enum_: &DataEnum,
    enum_name: &Ident,
    serde_crate_name: TokenStream,
) -> TokenStream {
    let variant_number = enum_.variants.len();
    let bits_needed = bits_needed_for(variant_number);

    let ser_method = get_ser_method(enum_, bits_needed);
    let de_method = get_de_method(enum_, bits_needed);
    let bit_length_method = get_bit_length_method(enum_, bits_needed);

    let lowercase_enum_name = Ident::new(
        enum_name.to_string().to_lowercase().as_str(),
        Span::call_site(),
    );
    let module_name = format_ident!("define_{}", lowercase_enum_name);

    let import_types =
        quote! { Serde, BitWrite, UnsignedInteger, BitReader, SerdeErr, ConstBitLength, };
    let imports = quote! { use #serde_crate_name::{#import_types}; };

    quote! {
        mod #module_name {
            #imports
            use super::#enum_name;

            impl Serde for #enum_name {
                #ser_method
                #de_method
                #bit_length_method
            }
        }
    }
}

fn get_ser_method(enum_: &DataEnum, bits_needed: u8) -> TokenStream {
    let mut ser = quote! {};
    for (index, variant) in enum_.variants.iter().enumerate() {
        let variant_index = index as u16;
        let variant_name = &variant.ident;
        let base = match &variant.fields {
            Fields::Unit => {
                quote! {
                    Self::#variant_name => {
                        let index = UnsignedInteger::<#bits_needed>::new(#variant_index);
                        index.ser(writer);
                    }
                }
            }
            Fields::Named(fields) => {
                let names: Vec<&Ident> = fields
                    .named
                    .iter()
                    .map(|field| {
                        field
                            .ident
                            .as_ref()
                            .expect("expected field to have a name.")
                    })
                    .collect();
                let left = quote! { Self::#variant_name{ #(#names),* } };
                let mut right = quote! {
                    let index = UnsignedInteger::<#bits_needed>::new(#variant_index);
                    index.ser(writer);
                };
                for field in fields.named.iter() {
                    let field_name = field
                        .ident
                        .as_ref()
                        .expect("expected field to have a name.");
                    right = quote! {
                        #right
                        #field_name.ser(writer);
                    }
                }
                quote! {
                    #left => { #right }
                }
            }
            Fields::Unnamed(fields) => {
                let names: Vec<Ident> = fields
                    .unnamed
                    .iter()
                    .enumerate()
                    .map(|(i, _)| format_ident!("f{}", i))
                    .collect();
                let left = quote! { Self::#variant_name( #(#names),* ) };

                let mut right = quote! {
                    let index = UnsignedInteger::<#bits_needed>::new(#variant_index);
                    index.ser(writer);
                };
                for field_name in names {
                    right = quote! {
                        #right
                        #field_name.ser(writer);
                    }
                }
                quote! {
                    #left => { #right }
                }
            }
        };
        ser = quote! {
            #ser
            #base
        }
    }
    quote! {
         fn ser(&self, writer: &mut dyn BitWrite) {
            match self {
                #ser
            }
         }
    }
}

fn get_de_method(enum_: &DataEnum, bits_needed: u8) -> TokenStream {
    let mut de = quote! {};

    for (index, variant) in enum_.variants.iter().enumerate() {
        let variant_index = index as u16;
        let variant_name = &variant.ident;
        match &variant.fields {
            Fields::Unit => {
                de = quote! {
                    #de
                    #variant_index => Self::#variant_name,
                }
            }
            Fields::Named(fields) => {
                let mut base = quote! {};
                for field in fields.named.iter() {
                    let field_name = field
                        .ident
                        .as_ref()
                        .expect("expected field to have a name.");
                    base = quote! {
                        #base
                        #field_name: Serde::de(reader)?,
                    }
                }
                de = quote! {
                    #de
                    #variant_index => Self::#variant_name{
                        #base
                    },
                }
            }
            Fields::Unnamed(fields) => {
                let mut base = quote! {};
                for _ in fields.unnamed.iter() {
                    base = quote! {
                        #base
                        Serde::de(reader)?,
                    }
                }
                de = quote! {
                    #de
                    #variant_index => Self::#variant_name(
                        #base
                    ),
                }
            }
        }
    }
    quote! {
        fn de(reader: &mut BitReader) -> std::result::Result<Self, SerdeErr> {
            let index: UnsignedInteger<#bits_needed> = Serde::de(reader)?;
            let index_u16: u16 = index.get() as u16;
            Ok(match index_u16 {
                #de
                _ => return Err(SerdeErr)
            })
        }
    }
}

fn get_bit_length_method(enum_: &DataEnum, bits_needed: u8) -> TokenStream {
    let mut bit_length = quote! {};
    for (_, variant) in enum_.variants.iter().enumerate() {
        let variant_name = &variant.ident;
        let base = match &variant.fields {
            Fields::Unit => {
                quote! {
                    Self::#variant_name => {
                        output += <UnsignedInteger::<#bits_needed> as ConstBitLength>::const_bit_length();
                    }
                }
            }
            Fields::Named(fields) => {
                let names: Vec<&Ident> = fields
                    .named
                    .iter()
                    .map(|field| {
                        field
                            .ident
                            .as_ref()
                            .expect("expected field to have a name.")
                    })
                    .collect();
                let left = quote! { Self::#variant_name{ #(#names),* } };
                let mut right = quote! {
                    output += <UnsignedInteger::<#bits_needed> as ConstBitLength>::const_bit_length();
                };
                for field in fields.named.iter() {
                    let field_name = field
                        .ident
                        .as_ref()
                        .expect("expected field to have a name.");
                    right = quote! {
                        #right
                        output += #field_name.bit_length();
                    }
                }
                quote! {
                    #left => { #right }
                }
            }
            Fields::Unnamed(fields) => {
                let names: Vec<Ident> = fields
                    .unnamed
                    .iter()
                    .enumerate()
                    .map(|(i, _)| format_ident!("f{}", i))
                    .collect();
                let left = quote! { Self::#variant_name( #(#names),* ) };

                let mut right = quote! {
                    output += <UnsignedInteger::<#bits_needed> as ConstBitLength>::const_bit_length();
                };
                for field_name in names {
                    right = quote! {
                        #right
                        output += #field_name.bit_length();
                    }
                }
                quote! {
                    #left => { #right }
                }
            }
        };
        bit_length = quote! {
            #bit_length
            #base
        }
    }
    quote! {
         fn bit_length(&self) -> u32 {
            let mut output = 0;
            match self {
                #bit_length
            }
            output
         }
    }
}
