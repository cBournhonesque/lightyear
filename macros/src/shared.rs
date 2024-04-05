use proc_macro2::{Ident, Span};
use syn::{Data, DeriveInput, Field, Fields, ItemEnum};

pub enum StructType {
    Struct,
    UnitStruct,
    TupleStruct,
}

/// Get the type of the struct
pub(crate) fn get_struct_type(input: &DeriveInput) -> StructType {
    if let Data::Struct(data_struct) = &input.data {
        return match &data_struct.fields {
            Fields::Named(_) => StructType::Struct,
            Fields::Unnamed(_) => StructType::TupleStruct,
            Fields::Unit => StructType::UnitStruct,
        };
    }
    panic!("Can only derive on a struct")
}

pub(crate) fn generate_unique_ident(prefix: &str) -> Ident {
    let uuid = uuid::Uuid::new_v4();
    let ident = format!("{}_{}", prefix, uuid).replace('-', "_");

    Ident::new(&ident, Span::call_site())
}

/// Get a copy of each enum field (including the attributes)
pub(crate) fn get_fields(input: &ItemEnum) -> Vec<Field> {
    let mut fields = Vec::new();
    for mut variant in input.variants.iter() {
        let Fields::Unnamed(ref unnamed) = variant.fields else {
            panic!("Field must be unnamed");
        };
        assert_eq!(unnamed.unnamed.len(), 1);
        let mut component = unnamed.unnamed.first().unwrap().clone();
        // get the attrs from the variant
        component.attrs = variant.attrs.clone();
        // set the field ident as the variant ident
        component.ident = Some(variant.ident.clone());
        // make field immutable
        fields.push(component);
    }
    fields
}

/// Make a copy of the input enum but remove all the field attributes defined by me
pub(crate) fn strip_attributes(input: &ItemEnum, attributes_to_remove: &[&str]) -> ItemEnum {
    let mut input = input.clone();
    for variant in input.variants.iter_mut() {
        // remove all attributes that are used in this macro
        variant.attrs.retain(|v| {
            v.path().segments.first().map_or(true, |s| {
                attributes_to_remove.iter().all(|attr| s.ident != *attr)
            })
        })
    }
    input
}
