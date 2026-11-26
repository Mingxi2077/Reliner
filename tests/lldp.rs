use reliner::lldp::*;
use serde_json::json;

fn expected() -> LldpExpectation {
    serde_json::from_value(json!({
        "local_port":"eth0", "capture_scope":"single_link_receive_only",
        "as_of":{"seconds":1700000010, "nanoseconds":0},
        "chassis":{"subtype":4,"value_hex":"020000000001"},
        "port":{"subtype":5,"value_hex":"45746831"}
    }))
    .unwrap()
}

const START: u32 = 1_700_000_000;

#[test]
fn ethernet_pcap_fcs_metadata_does_not_change_the_neighbor() {
    for (little, words) in [(true, 0u32), (false, 2u32)] {
        let mut payload = frame(b'1', 120);
        if words == 2 {
            payload.extend([0x2e, 0x97, 0xa5, 0x39]);
        }
        let mut bytes = pcap(little, false, &[(START, 0, payload)]);
        let link = 1 | (1 << 26) | (words << 28);
        bytes[20..24].copy_from_slice(&if little {
            link.to_le_bytes()
        } else {
            link.to_be_bytes()
        });
        assert_eq!(
            check_lldp_pcap(&bytes, &expected()).unwrap().status,
            LldpStatus::Match
        );
    }
}

#[test]
fn advertisements_for_bridges_beyond_the_link_do_not_identify_the_physical_peer() {
    let mut remote = frame(b'2', 120);
    remote[5] = 0;
    assert_eq!(
        check(&[(START, 0, remote.clone())]).unwrap().status,
        LldpStatus::MissingEvidence
    );
    remote[5] = 3;
    assert_eq!(
        check(&[(START, 0, frame(b'1', 120)), (START, 0, remote)])
            .unwrap()
            .status,
        LldpStatus::Match
    );
}

#[test]
fn file_reader_preserves_bytes_and_enforces_size_limit() -> std::io::Result<()> {
    use std::fs::{self, File};
    use std::io::{ErrorKind, Write};

    let path =
        std::env::temp_dir().join(format!("reliner-lldp-reader-{}.pcap", std::process::id()));
    let mut file = File::create_new(&path)?;
    let bytes = pcap(true, false, &[(START, 0, frame(b'1', 120))]);
    file.write_all(&bytes)?;
    assert_eq!(read_pcap(&path)?, bytes);
    let limit = 16 * 1024 * 1024;
    file.set_len(limit)?;
    assert_eq!(read_pcap(&path)?.len() as u64, limit);
    file.set_len(limit + 1)?;
    assert_eq!(read_pcap(&path).unwrap_err().kind(), ErrorKind::InvalidData);
    drop(file);
    fs::remove_file(&path)?;
    assert_eq!(read_pcap(&path).unwrap_err().kind(), ErrorKind::NotFound);
    Ok(())
}

fn frame(port: u8, ttl: u16) -> Vec<u8> {
    let mut f = vec![
        1, 128, 194, 0, 0, 14, 2, 0, 0, 0, 0, 1, 0x88, 0xcc, 2, 7, 4, 2, 0, 0, 0, 0, 1, 4, 5, 5,
        b'E', b't', b'h', port, 6, 2,
    ];
    f.extend(ttl.to_be_bytes());
    f.extend([0, 0]);
    f.resize(60, 0);
    f
}

fn pcap(little: bool, nano: bool, records: &[(u32, u32, Vec<u8>)]) -> Vec<u8> {
    let u32b = |v: u32| {
        if little {
            v.to_le_bytes()
        } else {
            v.to_be_bytes()
        }
    };
    let u16b = |v: u16| {
        if little {
            v.to_le_bytes()
        } else {
            v.to_be_bytes()
        }
    };
    let mut p = u32b(if nano { 0xa1b23c4d } else { 0xa1b2c3d4 }).to_vec();
    p.extend(u16b(2));
    p.extend(u16b(4));
    p.extend([0; 8]);
    p.extend(u32b(65535));
    p.extend(u32b(1));
    for (seconds, fraction, f) in records {
        p.extend(u32b(*seconds));
        p.extend(u32b(*fraction));
        p.extend(u32b(f.len() as u32));
        p.extend(u32b(f.len() as u32));
        p.extend(f);
    }
    p
}

#[test]
fn same_single_link_capture_corrected_expectation() {
    let bytes = pcap(true, false, &[(START, 0, frame(b'1', 120))]);
    let mut e = expected();
    e.port.value_hex = "45746832".into();
    let wrong = check_lldp_pcap(&bytes, &e).unwrap();
    assert_eq!(wrong.status, LldpStatus::WrongPeer);
    e.port.value_hex = "45746831".into();
    let right = check_lldp_pcap(&bytes, &e).unwrap();
    assert_eq!(right.status, LldpStatus::Match);
    assert_eq!(wrong.neighbors, right.neighbors);
    assert_eq!(right.local_port, "eth0");
}

fn check(records: &[(u32, u32, Vec<u8>)]) -> Result<LldpObservation, LldpError> {
    check_lldp_pcap(&pcap(true, false, records), &expected())
}

#[test]
fn timestamp_resolution_and_ttl_boundary_with_vlan() {
    for little in [false, true] {
        for nano in [false, true] {
            let mut f = frame(b'1', 10);
            f.splice(12..12, [0x81, 0, 0, 42]);
            let bytes = pcap(little, nano, &[(START, 1, f)]);
            let mut e = expected();
            let unit = if nano { 1 } else { 1000 };
            e.as_of.nanoseconds = unit - 1;
            let live = check_lldp_pcap(&bytes, &e).unwrap();
            assert_eq!(live.status, LldpStatus::Match);
            assert_eq!(live.neighbors[0].last_seen.nanoseconds, unit);
            e.as_of.nanoseconds = unit;
            let expired = check_lldp_pcap(&bytes, &e).unwrap();
            assert_eq!(expired.status, LldpStatus::MissingEvidence);
            assert_eq!(expired.neighbors[0].state, NeighborState::Expired);
        }
    }
}

#[test]
fn ambiguity_withdrawal_refresh_and_return() {
    let mut records = vec![
        (START, 0, frame(b'1', 5)),
        (START + 4, 0, frame(b'1', 10)),
        (START + 5, 0, frame(b'2', 20)),
    ];
    assert_eq!(check(&records).unwrap().status, LldpStatus::Ambiguous);
    records.push((START + 5, 1, frame(b'2', 0)));
    let one = check(&records).unwrap();
    assert_eq!(one.status, LldpStatus::Match);
    assert_eq!(one.neighbors.len(), 2);
    assert_eq!(one.neighbors[1].state, NeighborState::Withdrawn);
    records.push((START + 6, 0, frame(b'1', 0)));
    assert_eq!(check(&records).unwrap().status, LldpStatus::MissingEvidence);
    records.push((START + 7, 0, frame(b'2', 10)));
    assert_eq!(check(&records).unwrap().status, LldpStatus::WrongPeer);
}

#[test]
fn subtype_and_binary_value_both_identify_the_peer() {
    let mut f = frame(b'1', 120);
    f[25] = 7;
    f[26..30].copy_from_slice(&[0, 255, 128, 0]);
    let bytes = pcap(true, false, &[(START, 0, f)]);
    let mut e = expected();
    e.port.value_hex = "00ff8000".into();
    assert_eq!(
        check_lldp_pcap(&bytes, &e).unwrap().status,
        LldpStatus::WrongPeer
    );
    e.port.subtype = 7;
    assert_eq!(
        check_lldp_pcap(&bytes, &e).unwrap().status,
        LldpStatus::Match
    );
    e.chassis.subtype = 7;
    assert_eq!(
        check_lldp_pcap(&bytes, &e).unwrap().status,
        LldpStatus::WrongPeer
    );
    e.port.value_hex = "00FF8000".into();
    assert_eq!(
        check_lldp_pcap(&bytes, &e).unwrap_err(),
        LldpError::InvalidExpectation
    );
}

#[test]
fn non_lldp_and_optional_tlvs() {
    let mut other = frame(b'2', 0);
    other[12..14].copy_from_slice(&[8, 0]);
    for records in [vec![], vec![(START, 0, other.clone())]] {
        let absent = check(&records).unwrap();
        assert_eq!(absent.status, LldpStatus::MissingEvidence);
        assert!(absent.neighbors.is_empty());
    }
    let mut f = frame(b'1', 120);
    f.splice(34..34, [10, 3, b'a', b'b', b'c']);
    assert_eq!(
        check(&[(START, 0, other), (START, 1, f)]).unwrap().status,
        LldpStatus::Match
    );
}

#[test]
fn damaged_records_cannot_leave_a_wrong_peer_verdict() {
    use LldpError::*;
    let records = vec![(START, 0, frame(b'2', 120)), (START, 1, frame(b'1', 120))];
    let full = pcap(true, false, &records);
    for n in [0, 3, 4, 23, 25, 39, 41, 99, 101, 115, 117, full.len() - 1] {
        assert_eq!(
            check_lldp_pcap(&full[..n], &expected()).unwrap_err(),
            TruncatedData
        );
    }
    let error = |offset: usize, replacement: &[u8]| {
        let mut p = full.clone();
        p[offset..offset + replacement.len()].copy_from_slice(replacement);
        check_lldp_pcap(&p, &expected()).unwrap_err()
    };
    assert_eq!(error(0, &[10, 13, 13, 10]), UnsupportedFormat);
    assert_eq!(error(4, &[3]), UnsupportedFormat);
    assert_eq!(error(20, &[101]), UnsupportedFormat);
    assert_eq!(error(16, &[0, 0, 0, 0]), MalformedData);
    assert_eq!(error(32, &[59]), TruncatedData);
    assert_eq!(error(32, &[61]), MalformedData);
    assert_eq!(error(28, &1_000_000u32.to_le_bytes()), InvalidTimestamp);
    assert_eq!(error(100, &(START - 1).to_le_bytes()), InvalidTimestamp);
    assert_eq!(error(100, &(START + 11).to_le_bytes()), InvalidTimestamp);
    assert_eq!(error(116 + 14, &[4]), MalformedData);
    assert_eq!(error(116 + 16, &[9]), UnsupportedFormat);
    assert_eq!(error(116 + 30, &[6, 1]), MalformedData);
    assert_eq!(error(116 + 34, &[0, 1]), MalformedData);
    assert_eq!(error(116 + 34, &[2, 1]), MalformedData);
    assert_eq!(error(116 + 34, &[11, 255]), MalformedData);
    assert_eq!(
        check(&[(START, 0, frame(b'1', 120)[..34].to_vec())]).unwrap_err(),
        MalformedData
    );
}

#[test]
fn requires_port_scope_and_valid_evaluation_time() {
    let mut e = expected();
    let bytes = pcap(true, false, &[]);
    e.as_of.nanoseconds = 1_000_000_000;
    assert_eq!(
        check_lldp_pcap(&bytes, &e).unwrap_err(),
        LldpError::InvalidTimestamp
    );
    e.local_port.clear();
    assert_eq!(
        check_lldp_pcap(&bytes, &e).unwrap_err(),
        LldpError::InvalidExpectation
    );
    let mut v = serde_json::to_value(expected()).unwrap();
    v["capture_scope"] = json!("mixed");
    assert!(serde_json::from_value::<LldpExpectation>(v).is_err());
}

#[test]
fn chassis_tlv_is_independent_of_source_mac_and_port_subtype() {
    let mut f = frame(b'1', 120);
    f[6..12].copy_from_slice(&[2, 0, 0, 0, 0, 99]);
    // Network-address chassis ID carries an address-family octet before the IPv4 bytes.
    f.splice(14..23, [2, 6, 5, 1, 192, 0, 2, 1]);
    let bytes = pcap(false, false, &[(START, 0, f)]);
    let mut e = expected();
    e.chassis = LldpId {
        subtype: 5,
        value_hex: "01c0000201".into(),
    };
    assert_eq!(
        check_lldp_pcap(&bytes, &e).unwrap().status,
        LldpStatus::Match
    );
    e.chassis.value_hex = "01c0000202".into();
    assert_eq!(
        check_lldp_pcap(&bytes, &e).unwrap().status,
        LldpStatus::WrongPeer
    );
}
