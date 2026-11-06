//! Photon UDP framing, IPv4/IPv6 header stripping, CRC32 verification, and
//! fragmented command reassembly (`SendReliable`, `SendUnreliable`, `SendFragment`).

use crate::protocol16::{
    encode_event_data_body, encode_operation_request_body, encode_operation_response_body,
    EventDataPayload, OperationRequestPayload, OperationResponsePayload, Protocol16Reader,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Default Photon UDP ports used by Albion Online (`5056` game server, `5055` master, `4535` chat/auth).
pub const PHOTON_UDP_PORTS: [u16; 3] = [5056, 5055, 4535];

/// Official Albion Online server IPv4 prefixes (Americas/Washington, Asia/Singapore, Europe/Amsterdam).
pub const ALBION_SERVER_PREFIXES: [[u8; 3]; 3] = [[5, 45, 187], [5, 188, 125], [193, 169, 238]];

pub const PHOTON_HEADER_LEN: usize = 12;
pub const COMMAND_HEADER_LEN: usize = 12;
pub const FRAGMENT_HEADER_LEN: usize = 20;

const FLAG_ENCRYPTED: u8 = 0x01;
const FLAG_CRC: u8 = 0xCC;

const CMD_DISCONNECT: u8 = 4;
const CMD_SEND_RELIABLE: u8 = 6;
const CMD_SEND_UNRELIABLE: u8 = 7;
const CMD_SEND_FRAGMENT: u8 = 8;

const MSG_OPERATION_REQUEST: u8 = 2;
const MSG_OPERATION_RESPONSE: u8 = 3;
const MSG_EVENT_DATA: u8 = 4;

const MAX_PENDING_FRAGMENTS: usize = 64;
const MAX_FRAGMENT_TOTAL_LEN: usize = 512 * 1024;

/// Decoded high-level Photon message extracted from a reliable, unreliable, or reassembled command.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PhotonMessage {
    EventData(EventDataPayload),
    OperationRequest(OperationRequestPayload),
    OperationResponse(OperationResponsePayload),
}

/// Extracted UDP datagram slice from a raw IPv4 or IPv6 packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UdpDatagramSlice<'a> {
    pub src_port: u16,
    pub dst_port: u16,
    pub payload: &'a [u8],
}

/// Fast heuristic check (`len >= 3 && b[0] in {241, 242, 254}`)
/// or standard Photon peer headers with a sane `commandCount`.
#[inline]
pub fn looks_like_photon(payload: &[u8]) -> bool {
    if payload.len() < 3 {
        return false;
    }
    if matches!(payload[0], 241 | 242 | 254) {
        return true;
    }
    if payload.len() >= PHOTON_HEADER_LEN {
        let flags = payload[2];
        let cmd_count = payload[3];
        return (flags == 0 || flags == FLAG_CRC || flags == FLAG_ENCRYPTED)
            && (1..=64).contains(&cmd_count);
    }
    false
}

#[inline]
pub fn is_photon_port(port: u16) -> bool {
    PHOTON_UDP_PORTS.contains(&port)
}

/// Extracts the UDP ports and payload from a raw IPv4 or IPv6 packet (as delivered by `SIO_RCVALL`).
pub fn extract_udp_from_ip_packet(raw_ip: &[u8]) -> Option<UdpDatagramSlice<'_>> {
    if raw_ip.is_empty() {
        return None;
    }
    let version = raw_ip[0] >> 4;
    let udp_offset = match version {
        4 => {
            if raw_ip.len() < 20 {
                return None;
            }
            let ihl = ((raw_ip[0] & 0x0F) as usize) * 4;
            if ihl < 20 || raw_ip.len() < ihl + 8 {
                return None;
            }
            let protocol = raw_ip[9];
            if protocol != 17 {
                return None;
            }
            ihl
        }
        6 => {
            if raw_ip.len() < 40 + 8 {
                return None;
            }
            let mut next_header = raw_ip[6];
            let mut offset = 40usize;
            for _ in 0..8 {
                if next_header == 17 {
                    break;
                }
                // Hop-by-Hop (0), Routing (43), Destination Options (60)
                if matches!(next_header, 0 | 43 | 60) {
                    if raw_ip.len() < offset + 2 {
                        return None;
                    }
                    next_header = raw_ip[offset];
                    let hdr_ext_len = (raw_ip[offset + 1] as usize + 1) * 8;
                    offset = offset.checked_add(hdr_ext_len)?;
                } else if next_header == 44 {
                    // Fragment header (fixed 8 bytes)
                    if raw_ip.len() < offset + 8 {
                        return None;
                    }
                    next_header = raw_ip[offset];
                    offset = offset.checked_add(8)?;
                } else {
                    return None;
                }
            }
            if next_header != 17 || raw_ip.len() < offset + 8 {
                return None;
            }
            offset
        }
        _ => return None,
    };

    let udp = &raw_ip[udp_offset..];
    if udp.len() < 8 {
        return None;
    }
    let src_port = u16::from_be_bytes([udp[0], udp[1]]);
    let dst_port = u16::from_be_bytes([udp[2], udp[3]]);
    let udp_len = u16::from_be_bytes([udp[4], udp[5]]) as usize;
    if udp_len < 8 {
        return None;
    }
    let actual_end = udp_len.min(udp.len());
    Some(UdpDatagramSlice {
        src_port,
        dst_port,
        payload: &udp[8..actual_end],
    })
}

/// Computes the Photon CRC32 checksum (reflected polynomial `0xEDB88320`, initial `0xFFFF_FFFF`,
/// without final XOR, as used by Photon UDP `0xCC` headers).
pub fn crc32_photon(bytes: &[u8]) -> u32 {
    const POLY: u32 = 0xEDB8_8320;
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in bytes {
        crc ^= b as u32;
        for _ in 0..8 {
            if (crc & 1) != 0 {
                crc = (crc >> 1) ^ POLY;
            } else {
                crc >>= 1;
            }
        }
    }
    crc
}

#[derive(Debug, Clone)]
struct SegmentedPackage {
    total_length: usize,
    bytes_written: usize,
    data: Vec<u8>,
}

/// Stateful Photon UDP packet parser and fragment reassembler.
#[derive(Debug, Default, Clone)]
pub struct PhotonParser {
    pending_segments: BTreeMap<i32, SegmentedPackage>,
    pub packets_processed: u64,
    pub commands_processed: u64,
    pub crc_mismatches: u64,
}

impl PhotonParser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn pending_fragment_count(&self) -> usize {
        self.pending_segments.len()
    }

    /// Parses one raw UDP payload into zero or more [`PhotonMessage`]s.
    pub fn parse_udp_payload(&mut self, payload: &[u8]) -> Vec<PhotonMessage> {
        let mut out = Vec::new();
        if payload.len() < PHOTON_HEADER_LEN {
            return out;
        }

        let _peer_id = u16::from_be_bytes([payload[0], payload[1]]);
        let flags = payload[2];
        let command_count = payload[3] as usize;
        let _timestamp = u32::from_be_bytes([payload[4], payload[5], payload[6], payload[7]]);
        let _challenge = i32::from_be_bytes([payload[8], payload[9], payload[10], payload[11]]);

        // Encrypted handshake frames are ignored.
        if flags == FLAG_ENCRYPTED {
            return out;
        }

        let mut offset = PHOTON_HEADER_LEN;
        if flags == FLAG_CRC {
            if payload.len() < PHOTON_HEADER_LEN + 4 {
                return out;
            }
            let wire_crc = u32::from_be_bytes([
                payload[offset],
                payload[offset + 1],
                payload[offset + 2],
                payload[offset + 3],
            ]);
            let mut copy = payload.to_vec();
            copy[offset..offset + 4].fill(0);
            let computed = crc32_photon(&copy);
            if wire_crc != computed {
                self.crc_mismatches += 1;
                return out;
            }
            offset += 4;
        }

        self.packets_processed += 1;

        for _ in 0..command_count {
            if offset + COMMAND_HEADER_LEN > payload.len() {
                break;
            }
            let command_type = payload[offset];
            let _channel_id = payload[offset + 1];
            let _command_flags = payload[offset + 2];
            // offset + 3 is reserved byte
            let command_length = i32::from_be_bytes([
                payload[offset + 4],
                payload[offset + 5],
                payload[offset + 6],
                payload[offset + 7],
            ]);
            let _sequence_number = i32::from_be_bytes([
                payload[offset + 8],
                payload[offset + 9],
                payload[offset + 10],
                payload[offset + 11],
            ]);

            if command_length < COMMAND_HEADER_LEN as i32 {
                break;
            }
            let cmd_len = command_length as usize;
            let Some(cmd_end) = offset.checked_add(cmd_len) else {
                break;
            };
            if cmd_end > payload.len() {
                break;
            }

            let body = &payload[offset + COMMAND_HEADER_LEN..cmd_end];
            offset = cmd_end;
            self.commands_processed += 1;

            match command_type {
                CMD_DISCONNECT => break,
                CMD_SEND_RELIABLE => {
                    if let Some(msg) = Self::decode_reliable_body(body) {
                        out.push(msg);
                    }
                }
                CMD_SEND_UNRELIABLE => {
                    if body.len() > 4 {
                        if let Some(msg) = Self::decode_reliable_body(&body[4..]) {
                            out.push(msg);
                        }
                    }
                }
                CMD_SEND_FRAGMENT => {
                    if let Some(msg) = self.handle_fragment_body(body) {
                        out.push(msg);
                    }
                }
                _ => {}
            }
        }

        out
    }

    fn decode_reliable_body(body: &[u8]) -> Option<PhotonMessage> {
        if body.len() < 2 {
            return None;
        }
        let _signal_byte = body[0]; // 0xF3 or 0xFD
        let message_type = body[1];
        let mut reader = Protocol16Reader::new(&body[2..]);
        match message_type {
            MSG_OPERATION_REQUEST => reader
                .read_operation_request(0)
                .ok()
                .map(PhotonMessage::OperationRequest),
            MSG_OPERATION_RESPONSE => reader
                .read_operation_response(0)
                .ok()
                .map(PhotonMessage::OperationResponse),
            MSG_EVENT_DATA => reader.read_event_data(0).ok().map(PhotonMessage::EventData),
            _ => None,
        }
    }

    fn handle_fragment_body(&mut self, body: &[u8]) -> Option<PhotonMessage> {
        if body.len() < FRAGMENT_HEADER_LEN {
            return None;
        }
        let start_seq = i32::from_be_bytes([body[0], body[1], body[2], body[3]]);
        let _fragment_count = i32::from_be_bytes([body[4], body[5], body[6], body[7]]);
        let _fragment_number = i32::from_be_bytes([body[8], body[9], body[10], body[11]]);
        let total_length = i32::from_be_bytes([body[12], body[13], body[14], body[15]]);
        let fragment_offset = i32::from_be_bytes([body[16], body[17], body[18], body[19]]);

        if total_length <= 0 || fragment_offset < 0 {
            return None;
        }
        let total_len = total_length as usize;
        let frag_off = fragment_offset as usize;
        if total_len > MAX_FRAGMENT_TOTAL_LEN {
            return None;
        }

        let frag_slice = &body[FRAGMENT_HEADER_LEN..];
        let end_off = frag_off.checked_add(frag_slice.len())?;
        if end_off > total_len {
            return None;
        }

        if !self.pending_segments.contains_key(&start_seq)
            && self.pending_segments.len() >= MAX_PENDING_FRAGMENTS
        {
            if let Some(&oldest_key) = self.pending_segments.keys().next() {
                self.pending_segments.remove(&oldest_key);
            }
        }

        let entry = self
            .pending_segments
            .entry(start_seq)
            .or_insert_with(|| SegmentedPackage {
                total_length: total_len,
                bytes_written: 0,
                data: vec![0u8; total_len],
            });

        if entry.total_length != total_len {
            self.pending_segments.remove(&start_seq);
            return None;
        }

        entry.data[frag_off..end_off].copy_from_slice(frag_slice);
        entry.bytes_written = entry.bytes_written.saturating_add(frag_slice.len());

        if entry.bytes_written >= entry.total_length {
            let finished = self.pending_segments.remove(&start_seq)?;
            return Self::decode_reliable_body(&finished.data);
        }

        None
    }
}

/// Serializes a [`PhotonMessage`] into its inner reliable payload (`[0xF3, msgType, ...protocol16]`).
pub fn encode_reliable_message_body(msg: &PhotonMessage) -> Vec<u8> {
    let mut body = Vec::new();
    body.push(0xF3);
    match msg {
        PhotonMessage::OperationRequest(req) => {
            body.push(MSG_OPERATION_REQUEST);
            encode_operation_request_body(&mut body, req);
        }
        PhotonMessage::OperationResponse(resp) => {
            body.push(MSG_OPERATION_RESPONSE);
            encode_operation_response_body(&mut body, resp);
        }
        PhotonMessage::EventData(ev) => {
            body.push(MSG_EVENT_DATA);
            encode_event_data_body(&mut body, ev);
        }
    }
    body
}

/// Builds a wire-accurate Photon UDP packet containing one or more [`PhotonMessage`]s,
/// optionally protected by Photon CRC32 (`flags = 0xCC`).
pub fn build_photon_udp_packet(
    messages: &[PhotonMessage],
    with_crc: bool,
    unreliable: bool,
    base_sequence: i32,
) -> Vec<u8> {
    let mut pkt = Vec::new();
    pkt.extend_from_slice(&0x0001u16.to_be_bytes()); // peerId
    pkt.push(if with_crc { FLAG_CRC } else { 0 });
    pkt.push(messages.len() as u8);
    pkt.extend_from_slice(&123_456u32.to_be_bytes()); // timestamp
    pkt.extend_from_slice(&789_012i32.to_be_bytes()); // challenge
    if with_crc {
        pkt.extend_from_slice(&0u32.to_be_bytes()); // placeholder for CRC at 12..16
    }

    for (idx, msg) in messages.iter().enumerate() {
        let rel_body = encode_reliable_message_body(msg);
        let seq = base_sequence + idx as i32;
        if unreliable {
            let cmd_len = COMMAND_HEADER_LEN + 4 + rel_body.len();
            pkt.push(CMD_SEND_UNRELIABLE);
            pkt.push(0); // channel
            pkt.push(0); // flags
            pkt.push(0); // reserved
            pkt.extend_from_slice(&(cmd_len as i32).to_be_bytes());
            pkt.extend_from_slice(&seq.to_be_bytes());
            pkt.extend_from_slice(&(seq + 1000).to_be_bytes()); // unreliable seq
            pkt.extend_from_slice(&rel_body);
        } else {
            let cmd_len = COMMAND_HEADER_LEN + rel_body.len();
            pkt.push(CMD_SEND_RELIABLE);
            pkt.push(0);
            pkt.push(1);
            pkt.push(0);
            pkt.extend_from_slice(&(cmd_len as i32).to_be_bytes());
            pkt.extend_from_slice(&seq.to_be_bytes());
            pkt.extend_from_slice(&rel_body);
        }
    }

    if with_crc {
        let crc = crc32_photon(&pkt);
        pkt[12..16].copy_from_slice(&crc.to_be_bytes());
    }
    pkt
}

/// Splits a single [`PhotonMessage`] across multiple `SendFragment (8)` Photon UDP packets.
pub fn build_photon_fragmented_packets(
    msg: &PhotonMessage,
    chunk_size: usize,
    start_seq: i32,
    with_crc: bool,
) -> Vec<Vec<u8>> {
    let full = encode_reliable_message_body(msg);
    let total_len = full.len();
    let chunks: Vec<&[u8]> = full.chunks(chunk_size.max(1)).collect();
    let frag_count = chunks.len() as i32;
    let mut packets = Vec::with_capacity(chunks.len());
    let mut offset = 0usize;

    for (idx, slice) in chunks.into_iter().enumerate() {
        let mut pkt = Vec::new();
        pkt.extend_from_slice(&0x0001u16.to_be_bytes());
        pkt.push(if with_crc { FLAG_CRC } else { 0 });
        pkt.push(1); // 1 command per UDP datagram
        pkt.extend_from_slice(&123_456u32.to_be_bytes());
        pkt.extend_from_slice(&789_012i32.to_be_bytes());
        if with_crc {
            pkt.extend_from_slice(&0u32.to_be_bytes());
        }

        let cmd_len = COMMAND_HEADER_LEN + FRAGMENT_HEADER_LEN + slice.len();
        let seq = start_seq + idx as i32;
        pkt.push(CMD_SEND_FRAGMENT);
        pkt.push(0);
        pkt.push(1);
        pkt.push(0);
        pkt.extend_from_slice(&(cmd_len as i32).to_be_bytes());
        pkt.extend_from_slice(&seq.to_be_bytes());

        // 20-byte fragment header
        pkt.extend_from_slice(&start_seq.to_be_bytes());
        pkt.extend_from_slice(&frag_count.to_be_bytes());
        pkt.extend_from_slice(&(idx as i32).to_be_bytes());
        pkt.extend_from_slice(&(total_len as i32).to_be_bytes());
        pkt.extend_from_slice(&(offset as i32).to_be_bytes());
        pkt.extend_from_slice(slice);

        if with_crc {
            let crc = crc32_photon(&pkt);
            pkt[12..16].copy_from_slice(&crc.to_be_bytes());
        }

        offset += slice.len();
        packets.push(pkt);
    }

    packets
}

/// Wraps a UDP payload inside a minimal IPv4 + UDP header for testing raw socket extraction.
pub fn wrap_ipv4_udp_packet(
    src_ip: [u8; 4],
    dst_ip: [u8; 4],
    src_port: u16,
    dst_port: u16,
    udp_payload: &[u8],
) -> Vec<u8> {
    let udp_len = (8 + udp_payload.len()) as u16;
    let total_len = 20 + udp_len;
    let mut raw = Vec::with_capacity(total_len as usize);
    raw.push(0x45); // IPv4, IHL = 5 (20 bytes)
    raw.push(0x00);
    raw.extend_from_slice(&total_len.to_be_bytes());
    raw.extend_from_slice(&0u16.to_be_bytes());
    raw.extend_from_slice(&0u16.to_be_bytes());
    raw.push(64); // TTL
    raw.push(17); // Protocol = UDP
    raw.extend_from_slice(&0u16.to_be_bytes()); // checksum
    raw.extend_from_slice(&src_ip);
    raw.extend_from_slice(&dst_ip);
    // UDP header
    raw.extend_from_slice(&src_port.to_be_bytes());
    raw.extend_from_slice(&dst_port.to_be_bytes());
    raw.extend_from_slice(&udp_len.to_be_bytes());
    raw.extend_from_slice(&0u16.to_be_bytes());
    raw.extend_from_slice(udp_payload);
    raw
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol16::{ParameterTable, Protocol16Value};

    #[test]
    fn parses_crc_protected_reliable_and_unreliable_commands() {
        let mut params = ParameterTable::new();
        params.insert(0, Protocol16Value::Long(42));
        params.insert(252, Protocol16Value::Short(6));
        let msg = PhotonMessage::EventData(EventDataPayload {
            code: 1,
            parameters: params,
        });

        let udp = build_photon_udp_packet(std::slice::from_ref(&msg), true, true, 10);
        let raw_ip =
            wrap_ipv4_udp_packet([193, 169, 238, 10], [192, 168, 1, 20], 5056, 54321, &udp);
        let slice = extract_udp_from_ip_packet(&raw_ip).unwrap();
        assert_eq!(slice.src_port, 5056);
        assert!(is_photon_port(slice.src_port));

        let mut parser = PhotonParser::new();
        let decoded = parser.parse_udp_payload(slice.payload);
        assert_eq!(decoded, vec![msg]);
        assert_eq!(parser.crc_mismatches, 0);
    }

    #[test]
    fn rejects_corrupted_crc_packet() {
        let mut params = ParameterTable::new();
        params.insert(0, Protocol16Value::Long(99));
        let msg = PhotonMessage::EventData(EventDataPayload {
            code: 1,
            parameters: params,
        });
        let mut udp = build_photon_udp_packet(&[msg], true, false, 1);
        let last = udp.len() - 1;
        udp[last] ^= 0xFF;

        let mut parser = PhotonParser::new();
        let decoded = parser.parse_udp_payload(&udp);
        assert!(decoded.is_empty());
        assert_eq!(parser.crc_mismatches, 1);
    }

    #[test]
    fn reassembles_multi_fragment_photon_command() {
        let mut params = ParameterTable::new();
        params.insert(0, Protocol16Value::Long(1001));
        params.insert(1, Protocol16Value::String("A".repeat(450)));
        params.insert(253, Protocol16Value::Short(2));
        let msg = PhotonMessage::OperationResponse(OperationResponsePayload {
            operation_code: 1,
            return_code: 0,
            debug_message: None,
            parameters: params,
        });

        let fragments = build_photon_fragmented_packets(&msg, 96, 500, true);
        assert!(fragments.len() >= 5);

        let mut parser = PhotonParser::new();
        for (i, frag) in fragments.iter().enumerate() {
            let out = parser.parse_udp_payload(frag);
            if i + 1 < fragments.len() {
                assert!(out.is_empty());
                assert_eq!(parser.pending_fragment_count(), 1);
            } else {
                assert_eq!(out, vec![msg.clone()]);
                assert_eq!(parser.pending_fragment_count(), 0);
            }
        }
    }
}
