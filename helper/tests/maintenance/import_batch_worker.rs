use super::*;
use std::cell::Cell;

fn row() -> Row {
    Row {
        ordinal: 2,
        container: [2; 16],
        seed: [3; 16],
        node: [4; 16],
        requested: false,
        nonce: 0,
    }
}

#[test]
fn the_prior_receipt_needs_no_source_and_cannot_follow_a_source_request() {
    let prior = numbered(receipt(5, [2; 16], [3; 16], [4; 16]), 2);
    let mut previous = row();
    assert_eq!(
        previous.accept(prior).unwrap(),
        Next::Confirmed { prior: true }
    );
    assert!(!previous.requested);
    let mut wrong = row();
    assert!(wrong.accept(numbered(prior, 1)).is_err());
    for offset in [8, 9, 25, 41, 56] {
        let mut altered = prior;
        altered[offset] ^= 1;
        assert!(wrong.accept(altered).is_err(), "offset {offset}");
    }
    assert_eq!(wrong.accept(numbered(step(1, 0), 2)).unwrap(), Next::Source);
    assert!(wrong.accept(prior).is_err());
    assert!(wrong.accept(numbered(step(1, 0), 2)).is_err());
}

#[test]
fn a_new_receipt_needs_the_exact_source_request_and_monotonic_reproofs() {
    let mut current = row();
    let saved = numbered(receipt(6, [2; 16], [3; 16], [4; 16]), 2);
    assert!(current.accept(saved).is_err());
    assert!(current.accept(numbered(step(1, 0), 1)).is_err());
    current.accept(numbered(step(1, 0), 2)).unwrap();
    assert!(current.accept(saved).is_err());
    assert!(current.accept(numbered(step(3, 2), 2)).is_err());
    assert_eq!(
        current.accept(numbered(step(3, 1), 2)).unwrap(),
        Next::Recheck(1)
    );
    assert!(current.accept(numbered(step(3, 1), 2)).is_err());
    assert_eq!(
        current.accept(numbered(step(3, 2), 2)).unwrap(),
        Next::Recheck(2)
    );
    assert_eq!(
        current.accept(saved).unwrap(),
        Next::Confirmed { prior: false }
    );
}

#[test]
fn closing_the_batch_requires_exact_count_and_zero_reserved_bytes() {
    let end = step(8, 65_536);
    require_end(end, 65_536).unwrap();
    assert!(require_end(end, 65_535).is_err());
    for index in 9..65 {
        let mut malformed = end;
        malformed[index] = 1;
        assert!(require_end(malformed, 65_536).is_err());
    }
    assert!(require_end(step(7, 65_536), 65_536).is_err());
}

struct BrokenReport;

impl Write for BrokenReport {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Err(io::ErrorKind::BrokenPipe.into())
    }
}

#[test]
fn no_ack_on_lost_stdout_or_original_changed_during_output() {
    let acked = Cell::new(false);
    let ack = || {
        acked.set(true);
        Ok(())
    };
    let result = report_then_ack(
        &mut BrokenReport,
        |out| out.write_all(b"visible rows do not imply custody"),
        || Ok(()),
        ack,
    );
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::BrokenPipe);
    assert!(!acked.get());

    let cut = Cell::new(0);
    let result = report_then_ack(
        &mut Vec::new(),
        |out| out.write_all(b"receipt"),
        || {
            cut.set(cut.get() + 1);
            if cut.get() == 2 {
                Err(refused())
            } else {
                Ok(())
            }
        },
        ack,
    );
    assert!(result.is_err());
    assert_eq!(cut.get(), 2);
    assert!(!acked.get());

    let mut output = Vec::new();
    report_then_ack(&mut output, |out| out.write_all(b"receipt"), || Ok(()), ack).unwrap();
    assert_eq!(output, b"receipt");
    assert!(acked.get());
}
