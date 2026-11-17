use super::*;
use super::super::{MAX_RECORD_BYTES, Reply, encoded_fits};
use data_serialization::{Serializer, ser::SerializeSeq};
use std::cell::Cell;

#[test]
fn real_success_envelope_has_exact_compact_bytes_and_the_original_ceiling() {
    let make = |n| Reply::Success { result: format!("\0🧭\"{}", "a".repeat(n)) };
    let overhead = json_codec::to_vec(&make(0)).unwrap().len();
    let allowed = make(MAX_RECORD_BYTES - overhead);
    let source = json_codec::to_vec(&allowed).unwrap();
    assert_eq!(source.len(), MAX_RECORD_BYTES);
    assert!(encoded_fits(&allowed, MAX_RECORD_BYTES).unwrap());
    assert_eq!(frame(&allowed, MAX_RECORD_BYTES, || Failure::ReplyBudget {
        maximum: MAX_RECORD_BYTES,
    }).unwrap(), source);
    let oversized = make(MAX_RECORD_BYTES - overhead + 1);
    assert!(!encoded_fits(&oversized, MAX_RECORD_BYTES).unwrap());
    let dispatch = frame(&oversized, MAX_RECORD_BYTES, || Failure::DispatchBudget {
        maximum: MAX_RECORD_BYTES,
    }).unwrap_err();
    assert!(matches!(dispatch.downcast_ref::<Failure>(),
        Some(Failure::DispatchBudget { maximum: MAX_RECORD_BYTES })));
    let reply = frame(&oversized, MAX_RECORD_BYTES, || Failure::ReplyBudget {
        maximum: MAX_RECORD_BYTES,
    }).unwrap_err();
    assert!(matches!(reply.downcast_ref::<Failure>(),
        Some(Failure::ReplyBudget { maximum: MAX_RECORD_BYTES })));
}

#[test]
fn counting_and_collecting_stop_the_serializer_before_visiting_the_full_reply() {
    struct Rows<'a>(&'a Cell<usize>);
    impl Serialize for Rows<'_> {
        fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
            let mut seq = serializer.serialize_seq(None)?;
            for _ in 0..1_000_000 {
                self.0.set(self.0.get() + 1);
                seq.serialize_element("retained")?;
            }
            seq.end()
        }
    }
    let visited = Cell::new(0);
    let value = Reply::Success { result: Rows(&visited) };
    assert!(!encoded_fits(&value, 64).unwrap());
    let count = visited.replace(0);
    assert!((1..10).contains(&count));
    assert!(frame(&value, 64, || Failure::ReplyBudget { maximum: 64 }).is_err());
    assert_eq!(visited.get(), count);
}

#[test]
fn original_codec_failure_is_not_a_size_error() {
    struct Broken;
    impl Serialize for Broken {
        fn serialize<S: Serializer>(&self, _: S) -> std::result::Result<S::Ok, S::Error> {
            Err(data_serialization::ser::Error::custom("invalid original value"))
        }
    }
    assert!(encoded_fits(&Broken, MAX_RECORD_BYTES).unwrap_err().is::<json_codec::Error>());
    let err = frame(&Broken, MAX_RECORD_BYTES, || Failure::DispatchBudget {
        maximum: MAX_RECORD_BYTES,
    }).unwrap_err();
    assert!(err.is::<json_codec::Error>());
}

#[test]
fn long_service_failure_does_not_allocate_a_full_display_or_return_its_partial_prefix() {
    struct Streaming<'a>(&'a Cell<usize>);
    impl fmt::Display for Streaming<'_> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            for _ in 0..1_000_000 {
                self.0.set(self.0.get() + 1);
                f.write_str("local native detail ")?;
            }
            Ok(())
        }
    }
    let writes = Cell::new(0);
    let public = diagnostic(&Streaming(&writes));
    assert!(writes.get() < 300);
    assert!(!public.contains("local native detail"));
    assert!(public.contains("inspect the original operation"));
    let exact = "🧭".repeat(MAX_DIAGNOSTIC / 4);
    assert_eq!(diagnostic(&exact), exact);
}
