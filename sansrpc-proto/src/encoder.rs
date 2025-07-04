use crate::message::Message;

pub trait SansCoder<In, Out>: SansEncoder<Out> + SansDecoder<In> {}

impl<Coder, In, Out> SansCoder<In, Out> for Coder where Coder: SansEncoder<Out> + SansDecoder<In> {}

pub trait SansEncoder<T> {
    type Error;

    fn encode(&self, item: &Message<T>) -> Result<(Vec<u8>, u64), Self::Error>;
}

pub trait SansDecoder<T> {
    type Error;

    fn decode(&self, bytes: &[u8]) -> Result<Message<T>, Self::Error>;
}

pub trait IntoSplitCoder<In, Out>: SansCoder<In, Out> {
    type Encoder: SansEncoder<Out>;
    type Decoder: SansDecoder<In>;

    fn into_split(self) -> (Self::Decoder, Self::Encoder);
}

impl<Coder, In, Out> IntoSplitCoder<In, Out> for Coder
where
    Coder: Clone + SansCoder<In, Out>,
{
    type Encoder = Self;

    type Decoder = Self;

    fn into_split(self) -> (Self::Decoder, Self::Encoder) {
        (self.clone(), self)
    }
}
