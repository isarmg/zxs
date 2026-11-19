//! A future, explicitly provisioned CLI observation. No formatting, mounting,
//! devices created by the test, or reads of storage contents.
#[test]
fn one_explicit_kernel_device_has_separate_namespaces_for_ids_and_measured_rates(){
    let id=std::env::var("HELPER_TEST_BLOCK_DEV").expect("provide the major:minor of a stable test block device with readable kernel diskseq and statistics");
    let out=std::process::Command::new(env!("CARGO_BIN_EXE_helper"))
        .args(["disks","--device",&id,"--rates"]).output().unwrap();
    assert!(out.status.success(),"a steady, readable prepared device and thread mountinfo are required");
    let text=String::from_utf8(out.stdout).unwrap();let mut lines=text.lines();
    assert!(lines.next().unwrap().contains("rows=1 totals=not-summed health=not-checked"));
    let row=lines.next().unwrap();
    assert!(row.starts_with(&format!("dev={id} "))&&row.contains(" fs_uuid_udev_alias=")&&row.contains(" partuuid_udev_alias=")
        &&row.contains(" direct_mount_ids=")&&row.contains(" interval_ns=")&&row.contains(" read_Bps=")&&row.contains(" write_Bps="));
    assert!(lines.next().is_none());
}
