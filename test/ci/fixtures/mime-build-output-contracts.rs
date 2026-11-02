// Root test module for the actual private reverse-map build generator.
// Append this file at crate root after an exact byte-for-byte build.rs source prefix.
// Keep src/mime_types.rs at its original relative path beside that subject file.
// The private generator remains the original function; main() is never called.
// The validation gate appends it to the unchanged production build source.

#[cfg(not(feature = "rev-mappings"))]
compile_error!("this fixture requires rev-mappings; do not report a zero-test build as execution");

#[cfg(all(test, feature = "rev-mappings"))]
mod tests {
    use super::build_rev_map;
    use std::error::Error;
    use std::fmt;
    use std::io::{self, BufWriter, Write};

    const MAX_GENERATED_BYTES: usize = 1024 * 1024;

    fn generate<W: Write>(out: &mut W) -> io::Result<()> {
        build_rev_map(out)
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum FixtureFailure {
        MemoryBudget,
        BufferedFinalWrite,
        PrimaryWrite,
        FinalFlush,
    }

    impl FixtureFailure {
        fn kind(self) -> io::ErrorKind {
            match self {
                Self::PrimaryWrite => io::ErrorKind::PermissionDenied,
                Self::MemoryBudget | Self::BufferedFinalWrite | Self::FinalFlush => {
                    io::ErrorKind::Other
                }
            }
        }

        fn io_error(self) -> io::Error {
            io::Error::new(self.kind(), self)
        }
    }

    impl fmt::Display for FixtureFailure {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(formatter, "controlled fixture failure: {self:?}")
        }
    }

    impl Error for FixtureFailure {}

    struct ControlledWriter {
        bytes: Vec<u8>,
        fail_after: usize,
        write_failure: FixtureFailure,
        flush_failure: Option<FixtureFailure>,
        write_calls: usize,
        flush_calls: usize,
    }

    impl ControlledWriter {
        fn accepting() -> Self {
            Self {
                bytes: Vec::new(),
                fail_after: MAX_GENERATED_BYTES,
                write_failure: FixtureFailure::MemoryBudget,
                flush_failure: None,
                write_calls: 0,
                flush_calls: 0,
            }
        }
    }

    impl Write for ControlledWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.write_calls += 1;
            if bytes.is_empty() {
                return Ok(0);
            }
            if self.bytes.len() == self.fail_after {
                return Err(self.write_failure.io_error());
            }
            let accepted = bytes.len().min(self.fail_after - self.bytes.len());
            self.bytes.extend_from_slice(&bytes[..accepted]);
            Ok(accepted)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.flush_calls += 1;
            match self.flush_failure {
                Some(failure) => Err(failure.io_error()),
                None => Ok(()),
            }
        }
    }

    fn generated_bytes() -> Vec<u8> {
        let mut writer = ControlledWriter::accepting();
        generate(&mut writer).expect("the bounded accepting writer must generate a table");
        writer.bytes
    }

    fn assert_failure(result: io::Result<()>, expected: FixtureFailure) {
        let error = result.expect_err("the specific controlled error must be returned");
        assert_eq!(error.kind(), expected.kind());
        assert_eq!(
            error
                .get_ref()
                .and_then(|cause| cause.downcast_ref::<FixtureFailure>()),
            Some(&expected)
        );
    }

    #[test]
    fn accepting_writer_retains_generated_reverse_definitions() {
        let bytes = generated_bytes();
        assert!(!bytes.is_empty());
        let text = std::str::from_utf8(&bytes).expect("generated Rust source is UTF-8");
        assert!(text.starts_with("static REV_MAPPINGS:"));
        assert!(text.contains("\nconst EXTS:"));
        assert!(text.ends_with("];\n"));
    }

    #[test]
    fn successful_buffered_generation_flushes_before_returning() {
        let expected = generated_bytes();
        let inner = ControlledWriter::accepting();
        let mut writer = BufWriter::with_capacity(expected.len() + 1, inner);
        generate(&mut writer).expect("the accepting buffered writer must succeed");
        assert!(writer.buffer().is_empty());
        assert_eq!(writer.get_ref().flush_calls, 1);
        assert_eq!(writer.get_ref().bytes, expected);
    }

    #[test]
    fn buffered_final_write_failure_is_returned_with_its_original_cause() {
        let expected = generated_bytes();
        let inner = ControlledWriter {
            fail_after: 0,
            write_failure: FixtureFailure::BufferedFinalWrite,
            ..ControlledWriter::accepting()
        };
        // All generation fits in the buffer. The failing inner write can occur
        // only when the generator explicitly flushes its final buffered output.
        let mut writer = BufWriter::with_capacity(expected.len() + 1, inner);
        assert_failure(generate(&mut writer), FixtureFailure::BufferedFinalWrite);
        assert_eq!(writer.get_ref().write_calls, 1);
        assert_eq!(writer.get_ref().flush_calls, 0);
        // Check before Drop: BufWriter may retry its best-effort cleanup later.
    }

    #[test]
    fn partial_write_error_is_primary_and_prevents_a_later_flush() {
        let expected = generated_bytes();
        let mut writer = ControlledWriter {
            fail_after: 13,
            write_failure: FixtureFailure::PrimaryWrite,
            flush_failure: Some(FixtureFailure::FinalFlush),
            ..ControlledWriter::accepting()
        };
        assert_failure(generate(&mut writer), FixtureFailure::PrimaryWrite);
        assert_eq!(writer.bytes, expected[..13]);
        assert_eq!(writer.flush_calls, 0);
    }

    #[test]
    fn flush_failure_after_accepted_writes_is_returned_with_its_original_cause() {
        let expected = generated_bytes();
        let mut writer = ControlledWriter {
            flush_failure: Some(FixtureFailure::FinalFlush),
            ..ControlledWriter::accepting()
        };
        assert_failure(generate(&mut writer), FixtureFailure::FinalFlush);
        assert_eq!(writer.bytes, expected);
        assert_eq!(writer.flush_calls, 1);
    }
}
