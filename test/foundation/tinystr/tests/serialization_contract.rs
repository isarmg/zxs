use core::fmt;
use data_serialization::de::value::{SeqDeserializer, StrDeserializer};
use data_serialization::de::{DeserializeSeed, Error, SeqAccess, Visitor};
use data_serialization::{Deserialize, Deserializer};
use tinystr::TinyAsciiStr;

#[derive(Debug, PartialEq, Eq)]
struct TestError(String);

impl fmt::Display for TestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl core::error::Error for TestError {}

impl Error for TestError {
    fn custom<T: fmt::Display>(message: T) -> Self {
        Self(message.to_string())
    }
}

fn decode_str<const N: usize>(input: &str) -> Result<TinyAsciiStr<N>, TestError> {
    TinyAsciiStr::deserialize(StrDeserializer::new(input))
}

fn decode_tuple<const N: usize>(input: &[u8]) -> Result<TinyAsciiStr<N>, TestError> {
    TinyAsciiStr::deserialize(BinaryTuple(SeqDeserializer::new(input.iter().copied())))
}

struct BinaryTuple<A>(A);

impl<'de, A> Deserializer<'de> for BinaryTuple<A>
where
    A: SeqAccess<'de, Error = TestError>,
{
    type Error = TestError;

    fn deserialize_any<V>(self, _: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        Err(TestError("expected tuple deserialization".into()))
    }

    fn deserialize_tuple<V>(self, _: usize, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_seq(self.0)
    }

    fn is_human_readable(&self) -> bool {
        false
    }

    data_serialization::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 u8 u16 u32 u64 f32 f64 char str string
        bytes byte_buf option unit unit_struct newtype_struct seq
        tuple_struct map struct enum identifier ignored_any
    }
}

#[test]
fn human_error_preserves_length_and_capacity() {
    assert_eq!(
        decode_str::<4>("abcde").unwrap_err().to_string(),
        "found string of larger length 5 when constructing string of length 4"
    );
}

#[test]
fn human_error_preserves_null_cause() {
    assert_eq!(
        decode_str::<4>("a\0").unwrap_err().to_string(),
        "tinystr types do not support strings with null bytes"
    );
}

#[test]
fn human_error_preserves_non_ascii_cause() {
    assert_eq!(
        decode_str::<4>("é").unwrap_err().to_string(),
        "attempted to construct TinyAsciiStr from a non-ASCII string"
    );
}

#[test]
fn human_acceptance_is_unchanged() {
    assert_eq!(decode_str::<4>("").unwrap(), "");
    assert_eq!(decode_str::<4>("en").unwrap(), "en");
    assert_eq!(decode_str::<4>("Latn").unwrap(), "Latn");
}

#[test]
fn truncated_tuple_reports_the_number_of_consumed_elements() {
    for length in 0..4 {
        let input = vec![b'a'; length];
        assert_eq!(
            decode_tuple::<4>(&input).unwrap_err().to_string(),
            format!("invalid length {length}, expected a TinyAsciiStr<4>")
        );
    }
}

#[test]
fn tuple_acceptance_preserves_full_values_and_trailing_padding() {
    assert_eq!(decode_tuple::<0>(&[]).unwrap(), "");
    assert_eq!(decode_tuple::<4>(&[0; 4]).unwrap(), "");
    assert_eq!(decode_tuple::<4>(b"en\0\0").unwrap(), "en");
    assert_eq!(decode_tuple::<4>(b"Latn").unwrap(), "Latn");
}

#[test]
fn tuple_rejects_non_ascii_bytes() {
    assert_eq!(
        decode_tuple::<4>(&[b'a', 0x80, 0, 0])
            .unwrap_err()
            .to_string(),
        "TinyAsciiStr cannot contain non-ascii bytes"
    );
}

#[test]
fn tuple_rejects_non_trailing_null_bytes() {
    assert_eq!(
        decode_tuple::<4>(b"a\0b\0").unwrap_err().to_string(),
        "TinyAsciiStr cannot contain null bytes"
    );
}

#[test]
fn tuple_preserves_element_decoder_errors() {
    struct FailingSequence;

    impl<'de> SeqAccess<'de> for FailingSequence {
        type Error = TestError;

        fn next_element_seed<T>(&mut self, _: T) -> Result<Option<T::Value>, Self::Error>
        where
            T: DeserializeSeed<'de>,
        {
            Err(TestError("element decoder failed".into()))
        }
    }

    assert_eq!(
        TinyAsciiStr::<4>::deserialize(BinaryTuple(FailingSequence)).unwrap_err(),
        TestError("element decoder failed".into())
    );
}
