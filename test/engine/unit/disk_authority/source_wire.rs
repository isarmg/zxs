use super::*;

#[test]
fn bounded_output_accepts_exact_boundary_and_refuses_additional_bytes() {
    let mut writer = BoundedOutput(Vec::new());
    writer
        .write_all(&vec![b'x'; MAX_OBSERVATION_BYTES])
        .unwrap();
    assert_eq!(writer.0.len(), MAX_OBSERVATION_BYTES);
    assert_eq!(
        writer.write(b"!").unwrap_err().kind(),
        io::ErrorKind::FileTooLarge
    );
    assert_eq!(writer.0.len(), MAX_OBSERVATION_BYTES);
    writer.write_all(&[]).unwrap();
}

#[test]
fn bounded_output_rejects_an_oversized_chunk_without_partial_bytes() {
    let mut writer = BoundedOutput(Vec::new());
    writer.write_all(b"existing").unwrap();
    assert_eq!(
        writer
            .write(&vec![b'x'; MAX_OBSERVATION_BYTES])
            .unwrap_err()
            .kind(),
        io::ErrorKind::FileTooLarge
    );
    assert_eq!(writer.0, b"existing");
}
