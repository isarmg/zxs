use super::*;

fn header(status: u8, sense_len: u8) -> SgIo {
    SgIo {
        interface_id: i32::from(b'S'),
        direction: SG_DXFER_NONE,
        command_len: 16,
        sense_capacity: 64,
        iovec_count: 0,
        data_len: 0,
        data: std::ptr::null_mut(),
        command: std::ptr::null_mut(),
        sense: std::ptr::null_mut(),
        timeout_ms: 1,
        flags: 0,
        pack_id: 0,
        user: std::ptr::null_mut(),
        status,
        masked_status: 0,
        message_status: 0,
        sense_len,
        host_status: 0,
        driver_status: 0,
        residual: 0,
        duration: 0,
        info: 0,
    }
}

fn descriptor() -> [u8; 64] {
    let mut sense = [0; 64];
    sense[0] = 0x72;
    sense[1] = 1;
    sense[3] = 0x1d;
    sense[7] = 14;
    sense[8] = 9;
    sense[9] = 12;
    sense[13] = 0xff;
    sense[17] = 0x4f;
    sense[19] = 0xc2;
    sense[21] = 0x50;
    sense
}

#[test]
fn sat_command_set_contains_only_fixed_nondata_or_pio_reads() {
    for (command, opcode, feature, protocol, control) in [
        (Command::PowerMode, 0xe5, 0, 6, 0x2c),
        (Command::Health, 0xb0, 0xda, 6, 0x2c),
        (Command::Data(AtaData::Identify), 0xec, 0, 8, 0x0e),
        (Command::Data(AtaData::Attributes), 0xb0, 0xd0, 8, 0x0e),
        (Command::Data(AtaData::Thresholds), 0xb0, 0xd1, 8, 0x0e),
    ] {
        let cdb = command.cdb();
        assert_eq!(cdb[0], 0x85);
        assert_eq!(
            (cdb[1], cdb[2], cdb[4], cdb[14]),
            (protocol, control, feature, opcode)
        );
        assert_eq!(cdb[1] & 1, 0, "no 48-bit command extension");
        if matches!(command, Command::Data(_)) {
            assert_eq!(cdb[6], 1);
        }
        if opcode == 0xb0 {
            assert_eq!((cdb[10], cdb[12]), (0x4f, 0xc2));
        }
    }
    assert_eq!(Command::Data(AtaData::Thresholds).cdb()[8], 1);
}

#[test]
fn linux_sg_io_abi_layout_is_explicit() {
    assert_eq!(std::mem::size_of::<SgIo>(), 88);
    assert_eq!(std::mem::offset_of!(SgIo, data), 16);
    assert_eq!(std::mem::offset_of!(SgIo, timeout_ms), 40);
    assert_eq!(std::mem::offset_of!(SgIo, status), 64);
    assert_eq!(std::mem::offset_of!(SgIo, residual), 72);
}

#[test]
fn descriptor_and_fixed_sense_preserve_actual_ata_registers() {
    let expected = Registers {
        error: 0,
        status: 0x50,
        count: 0xff,
        lba_mid: 0x4f,
        lba_high: 0xc2,
    };
    assert_eq!(
        completion(&header(2, 22), &descriptor(), "fixture").unwrap(),
        Some(expected)
    );
    let mut fixed = [0; 64];
    fixed[0] = 0x70;
    fixed[2] = 1;
    fixed[4] = 0x50;
    fixed[6] = 0xff;
    fixed[7] = 10;
    fixed[10] = 0x4f;
    fixed[11] = 0xc2;
    fixed[13] = 0x1d;
    assert_eq!(
        completion(&header(2, 18), &fixed, "fixture").unwrap(),
        Some(expected)
    );
}

#[test]
fn malformed_partial_and_failed_completions_are_never_health() {
    assert!(completion(&header(2, 0), &[0; 64], "fixture").is_err());
    assert!(completion(&header(2, 65), &[0; 64], "fixture").is_err());
    for length in 0..22 {
        assert!(completion(&header(2, length), &descriptor(), "fixture").is_err());
    }
    let mut sense = descriptor();
    sense[21] = 0x51;
    assert!(matches!(
        completion(&header(2, 22), &sense, "fixture"),
        Err(Error::AtaStatus { .. })
    ));
    sense[21] = 0x50;
    sense[1] = 5;
    assert!(matches!(
        completion(&header(2, 22), &sense, "fixture"),
        Err(Error::ScsiSense { .. })
    ));
    let mut transport = header(0, 0);
    transport.residual = 1;
    assert!(completion(&transport, &[0; 64], "fixture").is_err());
    transport.residual = 0;
    transport.host_status = 3;
    assert!(matches!(
        completion(&transport, &[0; 64], "fixture"),
        Err(Error::ScsiStatus { host: 3, .. })
    ));
    transport.host_status = 1;
    assert!(matches!(
        completion(&transport, &[0; 64], "fixture"),
        Err(Error::ScsiStatus { .. })
    ));
    assert_eq!(
        completion(&header(0, 0), &[0; 64], "fixture").unwrap(),
        None
    );
}

#[test]
fn power_check_never_continues_for_low_power_unknown_or_missing_registers() {
    let mut registers = Registers {
        error: 0,
        status: 0x50,
        count: 0,
        lba_mid: 0,
        lba_high: 0,
    };
    for count in [0x00, 0x01, 0x40] {
        registers.count = count;
        assert_eq!(power_mode(Some(registers)).unwrap(), AtaPowerMode::LowPower);
    }
    for count in [0x41, 0x80, 0x81, 0x82, 0x83, 0xff] {
        registers.count = count;
        assert_eq!(power_mode(Some(registers)).unwrap(), AtaPowerMode::Ready);
    }
    registers.count = 0x7f;
    assert!(power_mode(Some(registers)).is_err());
    assert!(power_mode(None).is_err());
}

#[test]
fn malformed_health_signature_cannot_become_a_successful_attribute_fallback() {
    let mut registers = Registers {
        error: 0,
        status: 0x50,
        count: 0,
        lba_mid: 0x4f,
        lba_high: 0xc2,
    };
    assert_eq!(health_status(Some(registers)).unwrap(), AtaHealth::Passed);
    registers.lba_mid = 0xf4;
    registers.lba_high = 0x2c;
    assert_eq!(health_status(Some(registers)).unwrap(), AtaHealth::Failed);
    registers.lba_high = 0;
    assert!(health_status(Some(registers)).is_err());
    assert_eq!(
        health_status(None).unwrap(),
        AtaHealth::RegistersUnavailable
    );
}
