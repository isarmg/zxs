use super::*;
use crate::test_support::PrivateTemp;
use std::io::Write;

#[test]
fn a_short_inactive_phase_can_resume_but_never_appears_at_the_effective_name() {
    let temp = PrivateTemp::under(&std::env::temp_dir());
    let parent = HeldDir::trusted_storage(&temp.0).unwrap();
    let target = parent.new_child_directory(c"journal").unwrap();
    let before = (parent.entity, target.entity);
    let mut short = parent.create_opaque_record(c".phase.incoming").unwrap();
    short.write_all(b"ZXDM").unwrap();
    short.sync_all().unwrap();
    assert!(target.pin_record(c"SWITCHED").unwrap().is_none());
    let record = b"ZXDMS1\0\0example";
    let published = publish(
        (&parent, c".phase.incoming"),
        (&target, c"SWITCHED"),
        record,
        || {
            assert_eq!((parent.entity, target.entity), before);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(read_raw(&published, record.len()).unwrap(), record);
    assert!(parent.pin_record(c".phase.incoming").unwrap().is_none());
    let mut foreign = parent.create_opaque_record(c".other.incoming").unwrap();
    foreign.write_all(b"foreign").unwrap();
    foreign.sync_all().unwrap();
    assert!(
        publish(
            (&parent, c".other.incoming"),
            (&target, c"DONE"),
            b"ZXDMD1\0\0example",
            || Ok(())
        )
        .is_err()
    );
    let retained = pin(&parent, c".other.incoming").unwrap();
    assert_eq!(read_raw(&retained, 7).unwrap(), b"foreign");
    assert!(target.pin_record(c"DONE").unwrap().is_none());
}
