use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt::{self, Write};
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

pub fn read_pcap(path: &Path) -> io::Result<Vec<u8>> {
    const LIMIT: u64 = 16 * 1024 * 1024;
    let mut bytes = Vec::new();
    File::open(path)?.take(LIMIT + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > LIMIT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "PCAP exceeds 16 MiB",
        ));
    }
    Ok(bytes)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LldpExpectation {
    pub local_port: String,
    pub capture_scope: CaptureScope,
    pub as_of: LldpTimestamp,
    pub chassis: LldpId,
    pub port: LldpId,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureScope {
    SingleLinkReceiveOnly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LldpTimestamp {
    pub seconds: u64,
    pub nanoseconds: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LldpId {
    pub subtype: u8,
    pub value_hex: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LldpStatus {
    Match,
    WrongPeer,
    Ambiguous,
    MissingEvidence,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NeighborState {
    Active,
    Expired,
    Withdrawn,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LldpNeighbor {
    pub chassis: LldpId,
    pub port: LldpId,
    pub last_seen: LldpTimestamp,
    pub ttl_seconds: u16,
    pub state: NeighborState,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LldpObservation {
    pub local_port: String,
    pub capture_scope: CaptureScope,
    pub as_of: LldpTimestamp,
    pub status: LldpStatus,
    pub neighbors: Vec<LldpNeighbor>,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LldpError {
    InvalidExpectation,
    InvalidTimestamp,
    UnsupportedFormat,
    TruncatedData,
    MalformedData,
}

impl fmt::Display for LldpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidExpectation => "invalid local port or expected LLDP identifier",
            Self::InvalidTimestamp => {
                "invalid timestamp, out-of-order capture, or record after as_of"
            }
            Self::UnsupportedFormat => {
                "unsupported capture format, link type, or LLDP identifier subtype"
            }
            Self::TruncatedData => "incomplete capture header, record, or captured frame",
            Self::MalformedData => "invalid capture header, packet length, or protocol data",
        })
    }
}
impl std::error::Error for LldpError {}

fn id_length(kind: u8, subtype: u8, length: usize, family: Option<u8>) -> bool {
    if !(1..=255).contains(&length) {
        return false;
    }
    match (kind, subtype) {
        (1, 4) | (2, 3) => length == 6,
        (1, 5) | (2, 4) => match family {
            Some(1) => length == 5,
            Some(2) => length == 17,
            Some(0) | None => false,
            _ => length >= 2,
        },
        _ => true,
    }
}

fn valid_expected(kind: u8, id: &LldpId) -> bool {
    let s = &id.value_hex;
    (1..=7).contains(&id.subtype)
        && s.len() <= 510
        && s.len().is_multiple_of(2)
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        && id_length(
            kind,
            id.subtype,
            s.len() / 2,
            s.get(..2).and_then(|v| u8::from_str_radix(v, 16).ok()),
        )
}

fn wire_id(kind: u8, bytes: &[u8]) -> Result<LldpId, LldpError> {
    let (&subtype, value) = bytes.split_first().ok_or(LldpError::MalformedData)?;
    if !(1..=7).contains(&subtype) {
        return Err(LldpError::UnsupportedFormat);
    }
    if !id_length(kind, subtype, value.len(), value.first().copied()) {
        return Err(LldpError::MalformedData);
    }
    let mut value_hex = String::with_capacity(value.len() * 2);
    for byte in value {
        write!(value_hex, "{byte:02x}").unwrap();
    }
    Ok(LldpId { subtype, value_hex })
}

fn take<'a>(bytes: &mut &'a [u8], n: usize) -> Option<&'a [u8]> {
    let (part, rest) = bytes.split_at_checked(n)?;
    *bytes = rest;
    Some(part)
}

fn tlv<'a>(p: &mut &'a [u8]) -> Result<(u8, &'a [u8]), LldpError> {
    let h = take(p, 2).ok_or(LldpError::MalformedData)?;
    let bits = u16::from_be_bytes([h[0], h[1]]);
    Ok((
        (bits >> 9) as u8,
        take(p, (bits & 511) as usize).ok_or(LldpError::MalformedData)?,
    ))
}

fn mandatory<'a>(p: &mut &'a [u8], kind: u8) -> Result<&'a [u8], LldpError> {
    let (actual, value) = tlv(p)?;
    if actual != kind {
        return Err(LldpError::MalformedData);
    }
    Ok(value)
}

fn advertisement(mut p: &[u8]) -> Result<Option<(LldpId, LldpId, u16)>, LldpError> {
    let addresses = take(&mut p, 12).ok_or(LldpError::MalformedData)?;
    loop {
        match take(&mut p, 2).ok_or(LldpError::MalformedData)? {
            [0x81, 0] | [0x88, 0xa8] => {
                take(&mut p, 2).ok_or(LldpError::MalformedData)?;
            }
            [0x88, 0xcc] => break,
            _ => return Ok(None),
        }
    }
    // Provider/customer-scope LLDP can traverse intervening bridges. Only nearest-bridge
    // multicast supplies evidence for this single-link peer check.
    if addresses[..6] != [0x01, 0x80, 0xc2, 0x00, 0x00, 0x0e] {
        return Ok(None);
    }
    let chassis = wire_id(1, mandatory(&mut p, 1)?)?;
    let port = wire_id(2, mandatory(&mut p, 2)?)?;
    let ttl = u16::from_be_bytes(
        mandatory(&mut p, 3)?
            .try_into()
            .map_err(|_| LldpError::MalformedData)?,
    );
    loop {
        match tlv(&mut p)? {
            (0, []) => break,
            (0..=3, _) => return Err(LldpError::MalformedData),
            _ => {}
        }
    }
    Ok(Some((chassis, port, ttl)))
}

/// The caller attests that every frame was received on the named, single-link interface.
/// as_of is an explicit evaluation time at or after the final record.
pub fn check_lldp_pcap(
    input: &[u8],
    expectation: &LldpExpectation,
) -> Result<LldpObservation, LldpError> {
    use LldpError::*;
    let e = expectation;
    if e.local_port.trim().is_empty()
        || !valid_expected(1, &e.chassis)
        || !valid_expected(2, &e.port)
    {
        return Err(InvalidExpectation);
    }
    if e.as_of.nanoseconds >= 1_000_000_000 {
        return Err(InvalidTimestamp);
    }
    let mut bytes = input;
    let (little, scale) = match take(&mut bytes, 4).ok_or(TruncatedData)? {
        [0xd4, 0xc3, 0xb2, 0xa1] => (true, 1000),
        [0xa1, 0xb2, 0xc3, 0xd4] => (false, 1000),
        [0x4d, 0x3c, 0xb2, 0xa1] => (true, 1),
        [0xa1, 0xb2, 0x3c, 0x4d] => (false, 1),
        _ => return Err(UnsupportedFormat),
    };
    let h = take(&mut bytes, 20).ok_or(TruncatedData)?;
    let word = |s: &[u8]| {
        let b = s[..4].try_into().unwrap();
        if little {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        }
    };
    let link_info = word(&h[16..]);
    if h[..4] != (if little { [2, 0, 4, 0] } else { [0, 2, 0, 4] }) || link_info & 0xffff != 1 {
        return Err(UnsupportedFormat);
    }
    if link_info & 0x0bff_0000 != 0 {
        return Err(MalformedData);
    }
    // The P bit makes the FCS length explicit, in 16-bit words.
    let fcs_bytes = if link_info & (1 << 26) != 0 {
        (link_info >> 28) as usize * 2
    } else {
        0
    };
    let snaplen = word(&h[12..]);
    if snaplen == 0 {
        return Err(MalformedData);
    }
    let mut latest = BTreeMap::new();
    let mut previous = None;
    while !bytes.is_empty() {
        let h = take(&mut bytes, 16).ok_or(TruncatedData)?;
        let fraction = word(&h[4..]);
        if fraction >= 1_000_000_000 / scale {
            return Err(InvalidTimestamp);
        }
        let time = LldpTimestamp {
            seconds: word(h).into(),
            nanoseconds: fraction * scale,
        };
        if previous.is_some_and(|t| t > time) || time > e.as_of {
            return Err(InvalidTimestamp);
        }
        previous = Some(time);
        let cap = word(&h[8..]);
        let wire = word(&h[12..]);
        if cap > wire || cap > snaplen {
            return Err(MalformedData);
        }
        if cap < wire {
            return Err(TruncatedData);
        }
        let frame = take(&mut bytes, cap as usize).ok_or(TruncatedData)?;
        let frame = &frame[..frame.len().checked_sub(fcs_bytes).ok_or(MalformedData)?];
        if let Some((chassis, port, ttl)) = advertisement(frame)? {
            latest.insert((chassis, port), (time, ttl));
        }
    }
    let mut neighbors = Vec::with_capacity(latest.len());
    let mut status = LldpStatus::MissingEvidence;
    for ((chassis, port), (last_seen, ttl_seconds)) in latest {
        let expiry = LldpTimestamp {
            seconds: last_seen.seconds + u64::from(ttl_seconds),
            ..last_seen
        };
        let state = if ttl_seconds == 0 {
            NeighborState::Withdrawn
        } else if e.as_of >= expiry {
            NeighborState::Expired
        } else {
            status = match status {
                LldpStatus::MissingEvidence if chassis == e.chassis && port == e.port => {
                    LldpStatus::Match
                }
                LldpStatus::MissingEvidence => LldpStatus::WrongPeer,
                _ => LldpStatus::Ambiguous,
            };
            NeighborState::Active
        };
        neighbors.push(LldpNeighbor {
            chassis,
            port,
            last_seen,
            ttl_seconds,
            state,
        });
    }
    Ok(LldpObservation {
        local_port: e.local_port.clone(),
        capture_scope: e.capture_scope,
        as_of: e.as_of,
        status,
        neighbors,
    })
}
