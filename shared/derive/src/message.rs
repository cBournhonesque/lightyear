use darling::ast::{Data, NestedMeta};
use darling::{Error, FromDeriveInput, FromField, FromMeta};
use proc_macro2::{Ident, Span, TokenStream};
use quote::{format_ident, quote};
use syn::{
    parse_macro_input, parse_quote, DeriveInput, Field, Fields, Generics, ItemEnum, LitStr, Type,
    Variant,
};

#[derive(Debug, FromMeta)]
struct MacroAttrs {
    protocol: Ident,
}

const ATTRIBUTES: &'static [&'static str] = &["serialize"];

#[derive(Debug, FromDeriveInput)]
#[darling(attributes(serialize))]
struct SerializeDerive {
    // name of the struct
    ident: Ident,
    data: Data<Variant, Field>,

    #[darling(default)]
    serde: bool,
    #[darling(default)]
    bitcode: bool,
    #[darling(default)]
    custom: bool,
    #[darling(default)]
    nested: bool,
}

impl SerializeDerive {
    fn check_is_valid(&self) {
        let mut count = 0;
        if self.serde {
            count += 1;
        }
        if self.bitcode {
            count += 1;
        }
        if self.custom {
            count += 1;
        }
        if self.nested {
            count += 1;
        }
        if count != 1 {
            panic!(
                "The field {:?} cannot have multiple sync attributes set at the same time",
                self
            );
        }
    }
}

pub fn message_impl(
    input: proc_macro::TokenStream,
    shared_crate_name: TokenStream,
) -> proc_macro::TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let serialize_derive = match SerializeDerive::from_derive_input(&input) {
        Ok(v) => v,
        Err(e) => {
            return TokenStream::from(e.write_errors()).into();
        }
    };

    // Helper Properties
    let mut generics = input.generics.clone();
    generics.type_params_mut().for_each(|f| {
        // TODO: Disgusting... fix
        //  we need to add a bound here because some types implement Message only if the generic implements Message
        //  only implement the bounds that we need, we might not need DeserializeOwned if we use a custom serialize..
        if f.ident == "T" {
            // dbg!(&f);
            f.bounds.push(parse_quote! {
                Serialize
            });
            f.bounds.push(parse_quote! {
                DeserializeOwned
            });
            f.bounds.push(parse_quote! {
                Clone
            });
            f.bounds.push(parse_quote! {
                EventContext
            });
        }
    });
    let (impl_generics, type_generics, where_clause) = generics.split_for_impl();

    // Names
    let struct_name = input.ident;
    let struct_name_str = LitStr::new(&struct_name.to_string(), struct_name.span());
    let lowercase_struct_name = Ident::new(
        struct_name.to_string().to_lowercase().as_str(),
        Span::call_site(),
    );
    let module_name = format_ident!("define_{}", lowercase_struct_name);

    // Methods
    let bit_serialize_trait = bit_serialize_trait(&serialize_derive, &generics);

    let gen = quote! {
        mod #module_name {
            // use super::#struct_name;
            use super::*;
            use #shared_crate_name::{EventContext, Message, Named, Protocol};
            use #shared_crate_name::{BitSerializable, UserInput, WriteBuffer, ReadBuffer};

            impl #impl_generics Message for #struct_name #type_generics #where_clause {}

            #bit_serialize_trait

            // TODO: maybe we should just be able to convert a message into a MessageKind, and impl Display/Debug on MessageKind?
            impl #impl_generics Named for #struct_name #type_generics #where_clause {
                fn name(&self) -> String {
                    return #struct_name_str.to_string();
                }
            }
        }
        // use #module_name;
    };
    println!("{}", &gen);

    proc_macro::TokenStream::from(gen)
}

fn bit_serialize_trait(input: &SerializeDerive, generics: &Generics) -> TokenStream {
    if input.custom {
        // for custom serialization, let the user implement the traits themselves
        return quote! {};
    }
    let ident = &input.ident;

    let mut encode = quote! {};
    let mut decode = quote! {};
    let mut imports = quote! {};
    if input.bitcode {
        imports = quote! {
            use bitcode::{Encode, Decode};
            use bitcode::encoding::Fixed;
        };
        encode = quote! {
            writer.encode(&self, Fixed)
        };
        decode = quote! {
            reader.decode::<Self>(Fixed)
        };
    } else if input.nested {
        imports = quote! {
            use bitcode::encoding::Gamma;
        };
        // this is only valid for enums. We will call the encode/decode methods of each variant of the enum
        match &input.data {
            Data::Enum(e) => {
                let mut encode_variant = quote! {};
                let mut encode_body = quote! {};
                let mut decode_body = quote! {};

                for (i, variant) in e.iter().enumerate() {
                    let field = get_single_field(variant);
                    let field_ty = field.ty;
                    let variant_number = i as u8;
                    let variant_name = &variant.ident;
                    encode_variant = quote! {
                        #encode_variant
                        &#ident::#variant_name(_) => #variant_number,
                    };
                    encode_body = quote! {
                        #encode_body
                        &#ident::#variant_name(ref x) => x.encode(writer),
                    };
                    decode_body = quote! {
                        #decode_body
                        #variant_number => #ident::#variant_name(<#field_ty>::decode(reader)?),
                    }
                }
                encode = quote! {
                    #encode
                    let enum_number = match &self {
                        #encode_variant
                    };
                    writer.encode(&enum_number, Gamma)?;
                    match &self {
                        #encode_body
                    }
                };
                decode = quote! {
                    let enum_variant = reader.decode::<u8>(Gamma)?;
                    Ok(match enum_variant {
                        #decode_body
                    })
                };
            }
            Data::Struct(_) => panic!("Nested serialization is only valid for enums"),
        }
    } else {
        imports = quote! {
            use serde::{Serialize, Deserialize};
            use serde::de::DeserializeOwned;
        };
        // we use input.serde by default
        encode = quote! {
            writer.serialize(&self)
        };
        decode = quote! {
            reader.deserialize::<Self>()
        }
    }
    let (impl_generics, type_generics, where_clause) = generics.split_for_impl();
    quote! {
        #imports
        impl #impl_generics BitSerializable for #ident #type_generics #where_clause {
            fn encode(&self, writer: &mut impl WriteBuffer) -> anyhow::Result<()> {
                #encode
            }
            fn decode(reader: &mut impl ReadBuffer) -> anyhow::Result<Self>
            where Self: Sized {
                #decode
            }
        }
    }
}

pub fn message_protocol_impl(
    args: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
    shared_crate_name: TokenStream,
) -> proc_macro::TokenStream {
    let attr_args = match NestedMeta::parse_meta_list(args.into()) {
        Ok(v) => v,
        Err(e) => {
            return TokenStream::from(Error::from(e).write_errors()).into();
        }
    };
    let attr = match MacroAttrs::from_list(&attr_args) {
        Ok(v) => v,
        Err(e) => {
            return TokenStream::from(e.write_errors()).into();
        }
    };
    let protocol = &attr.protocol;
    let mut input = parse_macro_input!(input as ItemEnum);

    // Add extra variants
    // input.variants.push(parse_quote! {
    //     InputMessage(InputMessage<<#protocol as Protocol>::Input>)
    // });

    // Helper Properties
    let fields = get_fields(&input);

    // Names
    let enum_name = &input.ident;
    let lowercase_struct_name = Ident::new(
        enum_name.to_string().to_lowercase().as_str(),
        Span::call_site(),
    );
    let module_name = format_ident!("define_{}", lowercase_struct_name);
    let message_derive_name = if shared_crate_name.to_string() == "crate".to_string() {
        format_ident!("MessageInternal")
    } else {
        format_ident!("Message")
    };

    // Methods
    let add_events_method = add_events_method(&fields);
    let push_message_events_method = push_message_events_method(&fields, protocol);
    let name_method = name_method(&input);
    let encode_method = encode_method();
    let decode_method = decode_method();
    // let bit_serializable_trait = bit_serializable_trait(&input, &fields);

    let from_into_methods = from_into_methods(&input, &fields);

    let output = quote! {
        mod #module_name {
            use super::*;
            use serde::{Serialize, Deserialize};
            use bevy::prelude::{App, World};
            use #shared_crate_name::{enum_delegate, EnumAsInner};
            use #shared_crate_name::{ReadBuffer, WriteBuffer, BitSerializable, MessageBehaviour,
                MessageProtocol, MessageKind, Named};
            use #shared_crate_name::connection::events::{EventContext, IterMessageEvent};
            use #shared_crate_name::plugin::systems::events::push_message_events;
            use #shared_crate_name::plugin::events::MessageEvent;
            use #shared_crate_name::{InputMessage, UserInput, Protocol};
            use #shared_crate_name::BitSerializable;


            #[derive(Clone, Debug, PartialEq)]
            #[derive(#message_derive_name)]
            #[serialize(nested)]
            #[enum_delegate::implement(MessageBehaviour)]
            // #[derive(EnumAsInner)]
            #input

            impl MessageProtocol for #enum_name {
                type Protocol = #protocol;

                #add_events_method
                #push_message_events_method
            }

            // #bit_serializable_trait
            // #from_into_methods
            #name_method
            // impl BitSerializable for #enum_name {
            //     #encode_method
            //     #decode_method
            // }
        }
        pub use #module_name::#enum_name as #enum_name;

    };

    proc_macro::TokenStream::from(output)
}

fn push_message_events_method(fields: &Vec<Field>, protocol_name: &Ident) -> TokenStream {
    let mut body = quote! {};
    for field in fields {
        let message_type = &field.ty;
        body = quote! {
            #body
            push_message_events::<#message_type, #protocol_name, E, Ctx>(world, events);
        };
    }
    quote! {
        fn push_message_events<E: IterMessageEvent<#protocol_name, Ctx>, Ctx: EventContext>(
            world: &mut World,
            events: &mut E
        )
        {
            #body
        }
    }
}

fn bit_serializable_trait(input: &ItemEnum, fields: &Vec<Field>) -> TokenStream {
    let enum_name = &input.ident;
    let mut encode = quote! {};
    let mut encode_variant = quote! {};
    let mut decode = quote! {};
    for (i, field) in fields.iter().enumerate() {
        let variant_number = i as u8;
        let component_type = &field.ty;
        encode_variant = quote! {
            #encode_variant
            &#component_type(_) => variant_number,
        };
        encode = quote! {
            #encode
            &#component_type(ref x) => x.encode(writer),
        };
        decode = quote! {
            #decode
            variant_number => #component_type::decode(reader),
        };
    }
    quote! {
        impl BitSerializable for #enum_name {
            fn encode(&self, writer: &mut impl WriteBuffer) -> anyhow::Result<()> {
                let enum_number = match &self {
                    #encode_variant
                };
                writer.encode(enum_number, encoding::Gamma)?;
                match &self {
                    #encode
                }
            }
            fn decode(reader: &mut impl ReadBuffer) -> anyhow::Result<Self>
            where Self: Sized {
                let enum_variant = reader.decode::<u8>(encoding::Gamma)?;
                #decode
                match enum_variant {
                    #decode
                }
            }
        }
    }
}

fn add_events_method(fields: &Vec<Field>) -> TokenStream {
    let mut body = quote! {};
    for field in fields {
        let component_type = &field.ty;
        body = quote! {
            #body
            app.add_event::<MessageEvent<#component_type, Ctx>>();
        };
    }
    quote! {
        fn add_events<Ctx: EventContext>(app: &mut App)
        {
            #body
        }
    }
}

fn name_method(input: &ItemEnum) -> TokenStream {
    let enum_name = &input.ident;
    let variants = input.variants.iter().map(|v| v.ident.clone());
    let mut body = quote! {};
    for variant in input.variants.iter() {
        let ident = &variant.ident;
        body = quote! {
            #body
            &#enum_name::#ident(ref x) => x.name(),
        }
    }

    quote! {
        impl Named for #enum_name {
            fn name(&self) -> String {
                match self {
                    #body
                }
            }
        }
    }
}

fn from_into_methods(input: &ItemEnum, fields: &Vec<Field>) -> TokenStream {
    let enum_name = &input.ident;
    let variants = input.variants.iter().map(|v| v.ident.clone());
    let mut body = quote! {};
    for (variant, field) in input.variants.iter().zip(fields.iter()) {
        let ident = &variant.ident;
        body = quote! {
            #body
            impl From<#field> for #enum_name {
                fn from(value: #field) -> Self {
                    #enum_name::#ident(value)
                }
            }
            impl TryInto<#field> for #enum_name {
                type Error = ();
                fn try_into(self) -> Result<#field, Self::Error> {
                    match self {
                        #enum_name::#ident(x) => Ok(x),
                        _ => Err(()),
                    }
                }
            }
        }
    }

    quote! {
        #body
    }
}

fn encode_method() -> TokenStream {
    quote! {
        fn encode(&self, writer: &mut impl WriteBuffer) -> anyhow::Result<()> {
            writer.serialize(&self)
        }
    }
}

fn decode_method() -> TokenStream {
    quote! {
        fn decode(reader: &mut impl ReadBuffer) -> anyhow::Result<Self>
            where Self: Sized{
            reader.deserialize::<Self>()
        }
    }
}

fn get_single_field(variant: &Variant) -> Field {
    let Fields::Unnamed(unnamed) = &variant.fields else {
        panic!("Field must be unnamed");
    };
    assert_eq!(unnamed.unnamed.len(), 1);
    let mut field = unnamed.unnamed.first().unwrap().clone();
    // get the attrs from the variant
    field.attrs = variant.attrs.clone();
    field
}

fn get_fields(input: &ItemEnum) -> Vec<Field> {
    input
        .variants
        .iter()
        .map(|variant| get_single_field(variant))
        .collect()
}

/// Make a copy of the input enum but remove all the field attributes defined by me
fn strip_attributes(input: &ItemEnum) -> ItemEnum {
    let mut input = input.clone();
    for variant in input.variants.iter_mut() {
        // remove all attributes that are used in this macro
        variant.attrs.retain(|v| {
            v.path().segments.first().map_or(true, |s| {
                ATTRIBUTES
                    .iter()
                    .all(|attr| attr.to_string() != s.ident.to_string())
            })
        })
    }
    input
}
