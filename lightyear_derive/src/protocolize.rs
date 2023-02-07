use proc_macro2::{Punct, Spacing, Span, TokenStream};
use quote::{format_ident, quote};
use syn::{parse_macro_input, Data, DeriveInput, Ident, LitStr};

pub fn protocolize_impl(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = parse_macro_input!(input as DeriveInput);

    let protocol_name = input.ident;

    let variants = variants(&input.data);

    let name_method = name_method(&protocol_name, &variants);
    let kind_enum_name = format_ident!("{}Kind", protocol_name);
    let kind_enum_def = kind_enum(&kind_enum_name, &variants);
    let kind_of_method = kind_of_method();
    let type_to_kind_method = type_to_kind_method(&kind_enum_name, &variants);
    let dyn_ref_method = dyn_ref_method(&protocol_name, &variants);
    let dyn_mut_method = dyn_mut_method(&protocol_name, &variants);
    let cast_method = cast_method(&protocol_name, &variants);
    let clone_method = clone_method(&protocol_name, &variants);
    let cast_ref_method = cast_ref_method(&protocol_name, &variants);
    let cast_mut_method = cast_mut_method(&protocol_name, &variants);
    let extract_and_insert_method = extract_and_insert_method(&protocol_name, &variants);
    let write_method = write_method(&protocol_name, &variants);
    let write_update_method = write_update_method(&protocol_name, &variants);
    let read_method = read_method(&kind_enum_name, &variants);
    let read_create_update_method = read_create_update_method(&kind_enum_name, &variants);

    let gen = quote! {
        use std::{any::{Any, TypeId}, ops::{Deref, DerefMut}, sync::RwLock, collections::HashMap};
        use naia_shared::{ProtocolInserter, ProtocolKindType, ReplicateSafe, ComponentUpdate,
            DiffMask, ReplicaDynRef, ReplicaDynMut, Replicate, derive_serde,
            NetEntityConverter,
            serde, serde::{Serde, SerdeErr},
        };

        #kind_enum_def

        impl Protocolize for #protocol_name {
            type Kind = #kind_enum_name;
            #name_method
            #kind_of_method
            #type_to_kind_method
            #read_method
            #read_create_update_method
            #dyn_ref_method
            #dyn_mut_method
            #cast_method
            #cast_ref_method
            #cast_mut_method
            #extract_and_insert_method
            #write_method
            #write_update_method
        }

        impl Clone for #protocol_name {
            #clone_method
        }
    };

    proc_macro::TokenStream::from(gen)
}

pub fn variants(data: &Data) -> Vec<Ident> {
    let mut variants = Vec::new();
    if let Data::Enum(ref data) = *data {
        for variant in data.variants.iter() {
            variants.push(variant.ident.clone());
        }
    }
    variants
}

pub fn kind_enum(enum_name: &Ident, variants: &Vec<Ident>) -> TokenStream {
    let hashtag = Punct::new('#', Spacing::Alone);

    let mut variant_definitions = quote! {};
    let mut variants_to_type_id = quote! {};
    let mut variants_name = quote! {};

    for variant in variants {
        let variant_name = Ident::new(&variant.to_string(), Span::call_site());

        // Variant definitions
        {
            let new_output_right = quote! {
                #variant_name,
            };
            let new_output_result = quote! {
                #variant_definitions
                #new_output_right
            };
            variant_definitions = new_output_result;
        }

        // Variants to_type_id() match branch
        {
            let new_output_right = quote! {
                #enum_name::#variant_name => TypeId::of::<#variant_name>(),
            };
            let new_output_result = quote! {
                #variants_to_type_id
                #new_output_right
            };
            variants_to_type_id = new_output_result;
        }

        // Variants name() match branch
        {
            let variant_name_str = LitStr::new(&variant_name.to_string(), variant_name.span());
            let new_output_right = quote! {
                #enum_name::#variant_name => #variant_name_str.to_string(),
            };
            let new_output_result = quote! {
                #variants_name
                #new_output_right
            };
            variants_name = new_output_result;
        }
    }

    quote! {
        #hashtag[derive(Hash, Eq, Copy, Debug)]
        #hashtag[derive_serde]
        pub enum #enum_name {
            #variant_definitions
        }

        impl ProtocolKindType for #enum_name {
            fn to_type_id(&self) -> TypeId {
                match self {
                    #variants_to_type_id
                }
            }
            fn name(&self) -> String {
                match self {
                    #variants_name
                }
            }
        }
    }
}

fn kind_of_method() -> TokenStream {
    quote! {
        fn kind_of<R: ReplicateSafe<Self>>() -> Self::Kind {
            return Self::type_to_kind(TypeId::of::<R>()).expect("type not initialized correctly");
        }
    }
}

fn type_to_kind_method(enum_name: &Ident, variants: &Vec<Ident>) -> TokenStream {
    let mut insert_list = quote! {};

    {
        for variant_name in variants {
            let new_output_right = quote! {
                map.insert(TypeId::of::<#variant_name>(), #enum_name::#variant_name);
            };
            let new_output_result = quote! {
                #insert_list
                #new_output_right
            };
            insert_list = new_output_result;
        }
    }

    quote! {
        fn type_to_kind(type_id: TypeId) -> Option<Self::Kind> {
            unsafe {
                static mut TYPE_TO_KIND_MAP: Option<RwLock<HashMap<TypeId, #enum_name>>> = None;

                if TYPE_TO_KIND_MAP.is_none() {
                    let mut map: HashMap<TypeId, #enum_name> = HashMap::new();
                    #insert_list
                    TYPE_TO_KIND_MAP = Some((RwLock::new(map)));
                }

                return TYPE_TO_KIND_MAP
                    .as_ref()
                    .unwrap()
                    .read()
                    .unwrap()
                    .deref()
                    .get(&type_id)
                    .map(|kind| *kind);
            }
        }
    }
}

pub fn read_method(enum_name: &Ident, variants: &Vec<Ident>) -> TokenStream {
    let mut variants_build = quote! {};

    for variant in variants {
        let variant_name = Ident::new(&variant.to_string(), Span::call_site());

        // Variants build() match branch
        {
            let new_output_right = quote! {
                #enum_name::#variant_name => #variant_name::read(reader, converter),
            };
            let new_output_result = quote! {
                #variants_build
                #new_output_right
            };
            variants_build = new_output_result;
        }
    }

    quote! {
        fn read(reader: &mut serde::BitReader, converter: &dyn NetEntityConverter) -> Result<Self, SerdeErr> {
            let protocol_kind: Self::Kind = Self::Kind::de(reader)?;
            match protocol_kind {
                #variants_build
            }
        }
    }
}

pub fn read_create_update_method(enum_name: &Ident, variants: &Vec<Ident>) -> TokenStream {
    let mut variants_build = quote! {};

    for variant in variants {
        let variant_name = Ident::new(&variant.to_string(), Span::call_site());

        // Variants build() match branch
        {
            let new_output_right = quote! {
                #enum_name::#variant_name => #variant_name::read_create_update(reader),
            };
            let new_output_result = quote! {
                #variants_build
                #new_output_right
            };
            variants_build = new_output_result;
        }
    }

    quote! {
        fn read_create_update(reader: &mut serde::BitReader) -> Result<ComponentUpdate<Self::Kind>, SerdeErr> {
            let protocol_kind: Self::Kind = Self::Kind::de(reader)?;
            match protocol_kind {
                #variants_build
            }
        }
    }
}

pub fn dyn_ref_method(protocol_name: &Ident, variants: &Vec<Ident>) -> TokenStream {
    let mut variant_definitions = quote! {};

    for variant_name in variants {
        let new_output_right = quote! {
            #protocol_name::#variant_name(inner) => {
                return ReplicaDynRef::new(inner);
            }
        };
        let new_output_result = quote! {
            #variant_definitions
            #new_output_right
        };
        variant_definitions = new_output_result;
    }

    quote! {
        fn dyn_ref(&self) -> ReplicaDynRef<'_, Self> {
            match self {
                #variant_definitions
            }
        }
    }
}

pub fn dyn_mut_method(protocol_name: &Ident, variants: &Vec<Ident>) -> TokenStream {
    let mut variant_definitions = quote! {};

    for variant_name in variants {
        let new_output_right = quote! {
            #protocol_name::#variant_name(inner) => {
                return ReplicaDynMut::new(inner);
            }
        };
        let new_output_result = quote! {
            #variant_definitions
            #new_output_right
        };
        variant_definitions = new_output_result;
    }

    quote! {
        fn dyn_mut(&mut self) -> ReplicaDynMut<'_, Self> {
            match self {
                #variant_definitions
            }
        }
    }
}

pub fn cast_ref_method(protocol_name: &Ident, variants: &Vec<Ident>) -> TokenStream {
    let mut variant_definitions = quote! {};

    for variant_name in variants {
        let new_output_right = quote! {
            #protocol_name::#variant_name(replica_ref) => {
                let typed_ref = replica_ref as &dyn Any;
                return typed_ref.downcast_ref::<R>();
            }
        };
        let new_output_result = quote! {
            #variant_definitions
            #new_output_right
        };
        variant_definitions = new_output_result;
    }

    quote! {
        fn cast_ref<R: ReplicateSafe<Self>>(&self) -> Option<&R> {
            match self {
                #variant_definitions
            }
        }
    }
}

pub fn cast_mut_method(protocol_name: &Ident, variants: &Vec<Ident>) -> TokenStream {
    let mut variant_definitions = quote! {};

    for variant_name in variants {
        let new_output_right = quote! {
            #protocol_name::#variant_name(replica_ref) => {
                let typed_ref = replica_ref as &mut dyn Any;
                return typed_ref.downcast_mut::<R>();
            }
        };
        let new_output_result = quote! {
            #variant_definitions
            #new_output_right
        };
        variant_definitions = new_output_result;
    }

    quote! {
        fn cast_mut<R: ReplicateSafe<Self>>(&mut self) -> Option<&mut R> {
            match self {
                #variant_definitions
            }
        }
    }
}

pub fn cast_method(protocol_name: &Ident, variants: &Vec<Ident>) -> TokenStream {
    let mut variant_definitions = quote! {};

    for variant_name in variants {
        let new_output_right = quote! {
            #protocol_name::#variant_name(replica) => {
                let any_replica: &dyn Any = &replica;
                if let Some(any_ref) = any_replica.downcast_ref::<R>() {
                    return Some(any_ref.clone());
                }
            }
        };
        let new_output_result = quote! {
            #variant_definitions
            #new_output_right
        };
        variant_definitions = new_output_result;
    }

    quote! {
        fn cast<R: Replicate<Self>>(self) -> Option<R> {
            match self {
                #variant_definitions
            }

            return None;
        }
    }
}

pub fn clone_method(protocol_name: &Ident, variants: &Vec<Ident>) -> TokenStream {
    let mut variant_definitions = quote! {};

    for variant_name in variants {
        let new_output_right = quote! {
            #protocol_name::#variant_name(replica) => {
                return #protocol_name::#variant_name(replica.clone());
            }
        };
        let new_output_result = quote! {
            #variant_definitions
            #new_output_right
        };
        variant_definitions = new_output_result;
    }

    quote! {
        fn clone(&self) -> Self {
            match self {
                #variant_definitions
            }
        }
    }
}

fn extract_and_insert_method(type_name: &Ident, variants: &Vec<Ident>) -> TokenStream {
    let mut variant_definitions = quote! {};

    for variant_name in variants {
        let new_output_right = quote! {
            #type_name::#variant_name(replica_ref) => {
                inserter.insert(key, replica_ref.clone());
            }
        };
        let new_output_result = quote! {
            #variant_definitions
            #new_output_right
        };
        variant_definitions = new_output_result;
    }

    quote! {
        fn extract_and_insert<E, I: ProtocolInserter<#type_name, E>>(&self,
                                      key: &E,
                                      inserter: &mut I) {
            match self {
                #variant_definitions
            }
        }
    }
}

fn name_method(protocol_name: &Ident, variants: &Vec<Ident>) -> TokenStream {
    let mut variant_definitions = quote! {};

    for variant_name in variants {
        let new_output_right = quote! {
            #protocol_name::#variant_name(replica) => {
                return replica.name();
            }
        };
        let new_output_result = quote! {
            #variant_definitions
            #new_output_right
        };
        variant_definitions = new_output_result;
    }

    quote! {
        fn name(&self) -> String {
            match self {
                #variant_definitions
            }
        }
    }
}

fn write_method(protocol_name: &Ident, variants: &Vec<Ident>) -> TokenStream {
    let mut variant_definitions = quote! {};

    for variant_name in variants {
        let new_output_right = quote! {
            #protocol_name::#variant_name(replica) => {
                replica.write(writer, converter);
            }
        };
        let new_output_result = quote! {
            #variant_definitions
            #new_output_right
        };
        variant_definitions = new_output_result;
    }

    quote! {
        fn write(&self, writer: &mut dyn serde::BitWrite, converter: &dyn NetEntityConverter) {
            match self {
                #variant_definitions
            }
        }
    }
}

fn write_update_method(protocol_name: &Ident, variants: &Vec<Ident>) -> TokenStream {
    let mut variant_definitions = quote! {};

    for variant_name in variants {
        let new_output_right = quote! {
            #protocol_name::#variant_name(replica) => {
                replica.write_update(diff_mask, writer, converter);
            }
        };
        let new_output_result = quote! {
            #variant_definitions
            #new_output_right
        };
        variant_definitions = new_output_result;
    }

    quote! {
        fn write_update(&self, diff_mask: &DiffMask, writer: &mut dyn serde::BitWrite, converter: &dyn NetEntityConverter) {
            match self {
                #variant_definitions
            }
        }
    }
}
