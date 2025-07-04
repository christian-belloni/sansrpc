use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    encoder::{SansDecoder, SansEncoder},
    message::Message,
};

#[derive(Clone, Copy)]
pub struct BincodeSansCoder;

#[derive(Debug, Error)]
pub enum BincodeError {
    #[error(transparent)]
    Encode(#[from] bincode::error::EncodeError),
    #[error(transparent)]
    Decode(#[from] bincode::error::DecodeError),
}

impl<In: for<'a> Deserialize<'a>> SansDecoder<In> for BincodeSansCoder {
    type Error = BincodeError;

    fn decode(&self, bytes: &[u8]) -> Result<Message<In>, Self::Error> {
        Ok(bincode::serde::decode_from_slice(bytes, bincode::config::standard())?.0)
    }
}

impl<Out: Serialize> SansEncoder<Out> for BincodeSansCoder {
    type Error = BincodeError;

    fn encode(&self, item: &Message<Out>) -> Result<(Vec<u8>, u64), Self::Error> {
        let bytes = bincode::serde::encode_to_vec(item, bincode::config::standard())?;
        let len = bytes.len() as _;
        Ok((bytes, len))
    }
}
