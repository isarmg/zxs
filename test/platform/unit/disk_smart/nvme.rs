use super::*;

#[test]
fn linux_nvme_abi_layout_is_explicit() {
    assert_eq!(std::mem::size_of::<AdminCommand>(), 72);
    assert_eq!(std::mem::offset_of!(AdminCommand, address), 24);
    assert_eq!(std::mem::offset_of!(AdminCommand, data_len), 36);
    assert_eq!(std::mem::offset_of!(AdminCommand, cdw10), 40);
    assert_eq!(std::mem::offset_of!(AdminCommand, timeout_ms), 64);
    assert_eq!(NVME_IOCTL_ADMIN_CMD, 0xc048_4e41);
}

#[test]
fn controller_log_scope_follows_identify_lpa() {
    let mut data = [0; 4096];
    assert!(parse_controller(&data).is_err());
    data[0] = 1;
    assert_eq!(
        parse_controller(&data).unwrap().health_namespace(7),
        u32::MAX
    );
    data[261] = 1;
    assert_eq!(parse_controller(&data).unwrap().health_namespace(7), 7);
    assert!(parse_controller(&[0xff; 4096]).is_err());
}

#[test]
fn nvme_health_preserves_critical_bits_zero_metrics_and_u128_overflow() {
    let mut data = [0; 512];
    assert_eq!(
        parse_health(&data).unwrap(),
        NvmeHealth {
            critical_warning: 0,
            temperature_celsius: None,
            power_on_hours: 0,
        }
    );
    data[0] = 0x80;
    data[1..3].copy_from_slice(&273_u16.to_le_bytes());
    data[128..136].copy_from_slice(&u64::MAX.to_le_bytes());
    assert_eq!(
        parse_health(&data).unwrap(),
        NvmeHealth {
            critical_warning: 0x80,
            temperature_celsius: Some(0),
            power_on_hours: u64::MAX,
        }
    );
    data[136] = 1;
    assert!(matches!(
        parse_health(&data),
        Err(Error::InvalidResponse { .. })
    ));
}
