use serde::ser;


pub struct EntitySerializer {


}


// we use reflect to
// - serialize a component/message without knowing its exact type
// - deserialize a component/message without knowing its exact type


// each component/message will need to
// - derive Serialize/Deserialize, which means they can be serialized into the Serde data model using a custom serializer

// we will have a custom Serializer/Deserializer that:
// - wraps an existing Serializer/Deserializer (for example bincode)
// - efficiently serializes Entities into NetEntities



// we want to implement Serde's Serialize on our Protocol. (or on Components and Messages)
// we will map the protocol to the Serde Data Model by representing it as an Enum
// so that the serializers will then write the discriminant, etc.


// so we need to
// 1) upgrade naia-serde to implement Serializer/Deserializer
     // it will contain an internal BitWriter maybe? and a dyn EntityConverter
     // it will serialize the entities using the dyn converter
// 2) make sure Protocol implements Serialize to be represented as an Enum
// 3) maybe we need
