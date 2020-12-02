/// Coin defines a token with a denomination and an amount.
///
/// NOTE: The amount field is an Int which implements the custom method
/// signatures required by gogoproto.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Coin {
    #[prost(string, tag = "1")]
    pub denom: std::string::String,
    #[prost(string, tag = "2")]
    pub amount: std::string::String,
}
/// DecCoin defines a token with a denomination and a decimal amount.
///
/// NOTE: The amount field is an Dec which implements the custom method
/// signatures required by gogoproto.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct DecCoin {
    #[prost(string, tag = "1")]
    pub denom: std::string::String,
    #[prost(string, tag = "2")]
    pub amount: std::string::String,
}
/// IntProto defines a Protobuf wrapper around an Int object.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct IntProto {
    #[prost(string, tag = "1")]
    pub int: std::string::String,
}
/// DecProto defines a Protobuf wrapper around a Dec object.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct DecProto {
    #[prost(string, tag = "1")]
    pub dec: std::string::String,
}