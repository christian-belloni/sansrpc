use crate::{
    encoder::{SansDecoder, SansEncoder},
    message::Message,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy)]
pub struct JsonSansCoder;

impl<Out: Serialize> SansEncoder<Out> for JsonSansCoder {
    type Error = serde_json::Error;

    fn encode(&self, item: &Message<Out>) -> Result<(Vec<u8>, u64), Self::Error> {
        let bytes = serde_json::to_vec(item)?;
        let len = bytes.len() as _;
        Ok((bytes, len))
    }
}

impl<In: for<'a> Deserialize<'a>> SansDecoder<In> for JsonSansCoder {
    type Error = serde_json::Error;

    fn decode(&self, bytes: &[u8]) -> Result<Message<In>, Self::Error> {
        serde_json::from_slice(bytes)
    }
}
