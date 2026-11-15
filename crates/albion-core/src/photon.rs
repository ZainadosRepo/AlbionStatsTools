//! Photon UDP framing, IPv4/IPv6 header stripping, IPv4 fragment reassembly,
//! CRC32 verification, Protocol18 message decoding, and fragmented command
//! reassembly (`SendReliable`, `SendUnreliable`, `SendFragment`).

use crate::protocol16::{EventDataPayload, OperationRequestPayload, OperationResponsePayload};
use crate::protocol18::{
    encode_protocol18_event_data_body, encode_protocol18_operation_request_body,
    encode_protocol18_operation_response_body, Protocol18Reader,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

/// Default Photon UDP ports used by Albion Online (`5056` game server, `5055` master, `5058` world/chat, `4535` relay).
pub const PHOTON_UDP_PORTS: [u16; 4] = [5056, 5055, 5058, 4535];

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

const MAX_PENDING_FRAGMENTS: usize = 256;
const MAX_FRAGMENT_TOTAL_LEN: usize = 16 * 1024 * 1024;
const MAX_FRAGMENT_COUNT: usize = 16 * 1024;
const MAX_PENDING_FRAGMENT_BYTES: usize = 64 * 1024 * 1024;

const MAX_PENDING_IPV4_FRAGMENTS: usize = 256;
const MAX_IPV4_PAYLOAD_LEN: usize = 65_535;
const IPV4_FRAGMENT_TIMEOUT_MS: u64 = 15_000;
const IPV4_FRAGMENT_CLEANUP_INTERVAL_MS: u64 = 1_000;

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

/// Reassembled UDP datagram owning its payload buffer (used for fragmented IPv4 datagrams).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedUdpDatagram {
    pub src_port: u16,
    pub dst_port: u16,
    pub payload: Vec<u8>,
}

/// Validates and determines the byte length of the next Photon UDP packet inside `payload`
/// (supporting multiple coalesced Photon peer frames inside a single UDP datagram).
pub fn try_read_photon_packet_length(payload: &[u8]) -> Option<usize> {
    if payload.len() < PHOTON_HEADER_LEN {
        return None;
    }

    let flags = payload[2];
    if flags == FLAG_ENCRYPTED {
        return Some(payload.len());
    }

    let mut offset = PHOTON_HEADER_LEN + if flags == FLAG_CRC { 4 } else { 0 };
    if offset > payload.len() {
        return None;
    }

    let command_count = payload[3] as usize;
    for _ in 0..command_count {
        if offset + COMMAND_HEADER_LEN > payload.len() {
            return None;
        }
        let command_length = i32::from_be_bytes([
            payload[offset + 4],
            payload[offset + 5],
            payload[offset + 6],
            payload[offset + 7],
        ]);
        if command_length < COMMAND_HEADER_LEN as i32
            || (command_length as usize) > (payload.len() - offset)
        {
            return None;
        }
        offset += command_length as usize;
    }

    Some(offset)
}

/// Fast heuristic check (`len >= 3 && b[0] in {241, 242, 254}`)
/// or standard/coalesced Photon peer headers with verified command framing.
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
        if (flags == 0 || flags == FLAG_CRC || flags == FLAG_ENCRYPTED)
            && (1..=64).contains(&cmd_count)
            && try_read_photon_packet_length(payload).is_some()
        {
            return true;
        }
    }
    false
}

#[inline]
pub fn is_photon_port(port: u16) -> bool {
    PHOTON_UDP_PORTS.contains(&port)
        || (5055..=5060).contains(&port)
        || (4530..=4540).contains(&port)
}

/// Extracts the UDP ports and payload from an unfragmented raw IPv4 or IPv6 packet.
///
/// Fragmented IPv4/IPv6 packets return `None` so callers can feed IPv4 fragments through
/// [`Ipv4FragmentAssembler`] instead of misreading trailing IP fragments as UDP payloads.
pub fn extract_udp_from_ip_packet(raw_ip: &[u8]) -> Option<UdpDatagramSlice<'_>> {
    if raw_ip.is_empty() {
        return None;
    }
    let version = raw_ip[0] >> 4;
    let (scoped_ip, udp_offset) = match version {
        4 => {
            if raw_ip.len() < 20 {
                return None;
            }
            let ihl = ((raw_ip[0] & 0x0F) as usize) * 4;
            let total_len = u16::from_be_bytes([raw_ip[2], raw_ip[3]]) as usize;
            if ihl < 20 || total_len < ihl + 8 || total_len > raw_ip.len() {
                return None;
            }
            let flags_frag = u16::from_be_bytes([raw_ip[6], raw_ip[7]]);
            // Reject fragmented IPv4 packets (`MF` bit 0x2000 or non-zero fragment offset 0x1FFF).
            if (flags_frag & 0x3FFF) != 0 {
                return None;
            }
            let protocol = raw_ip[9];
            if protocol != 17 {
                return None;
            }
            (&raw_ip[..total_len], ihl)
        }
        6 => {
            if raw_ip.len() < 40 + 8 {
                return None;
            }
            let payload_len = u16::from_be_bytes([raw_ip[4], raw_ip[5]]) as usize;
            let packet_end = if payload_len > 0 {
                let end = 40usize.checked_add(payload_len)?;
                if end > raw_ip.len() {
                    return None;
                }
                end
            } else {
                raw_ip.len()
            };
            let scoped = &raw_ip[..packet_end];

            let mut next_header = scoped[6];
            let mut offset = 40usize;
            for _ in 0..8 {
                if next_header == 17 {
                    break;
                }
                // Hop-by-Hop (0), Routing (43), Destination Options (60)
                if matches!(next_header, 0 | 43 | 60) {
                    if scoped.len() < offset + 2 {
                        return None;
                    }
                    next_header = scoped[offset];
                    let hdr_ext_len = (scoped[offset + 1] as usize + 1) * 8;
                    offset = offset.checked_add(hdr_ext_len)?;
                } else if next_header == 44 {
                    // Fragment header (fixed 8 bytes)
                    if scoped.len() < offset + 8 {
                        return None;
                    }
                    let frag_field = u16::from_be_bytes([scoped[offset + 2], scoped[offset + 3]]);
                    // Reject if fragment offset (top 13 bits) or M-flag (LSB) is non-zero.
                    if (frag_field & 0xFFF9) != 0 {
                        return None;
                    }
                    next_header = scoped[offset];
                    offset = offset.checked_add(8)?;
                } else {
                    return None;
                }
            }
            if next_header != 17 || scoped.len() < offset + 8 {
                return None;
            }
            (scoped, offset)
        }
        _ => return None,
    };

    let udp = &scoped_ip[udp_offset..];
    if udp.len() < 8 {
        return None;
    }
    let src_port = u16::from_be_bytes([udp[0], udp[1]]);
    let dst_port = u16::from_be_bytes([udp[2], udp[3]]);
    let udp_len = u16::from_be_bytes([udp[4], udp[5]]) as usize;
    if udp_len < 8 || udp_len > udp.len() {
        return None;
    }
    Some(UdpDatagramSlice {
        src_port,
        dst_port,
        payload: &udp[8..udp_len],
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Ipv4FragmentKey {
    src_ip: [u8; 4],
    dst_ip: [u8; 4],
    identification: u16,
    protocol: u8,
}

#[derive(Debug, Clone)]
struct Ipv4FragmentAssembly {
    payload: Vec<u8>,
    received: Vec<bool>,
    received_count: usize,
    total_length: Option<usize>,
    last_seen_ms: u64,
}

impl Ipv4FragmentAssembly {
    fn new(now_ms: u64) -> Self {
        Self {
            payload: Vec::new(),
            received: Vec::new(),
            received_count: 0,
            total_length: None,
            last_seen_ms: now_ms,
        }
    }

    fn insert_fragment(&mut self, offset: usize, data: &[u8], has_more: bool, now_ms: u64) -> bool {
        let Some(end) = offset.checked_add(data.len()) else {
            return false;
        };
        if data.is_empty() || end > MAX_IPV4_PAYLOAD_LEN {
            return false;
        }
        if let Some(total) = self.total_length {
            if (!has_more && end != total) || (has_more && end > total) {
                return false;
            }
        }
        if !has_more {
            if end < self.payload.len() && self.received[end..].iter().any(|&b| b) {
                return false;
            }
            self.total_length = Some(end);
        }

        if end > self.payload.len() {
            self.payload.resize(end, 0);
            self.received.resize(end, false);
        }
        self.payload[offset..end].copy_from_slice(data);
        for slot in &mut self.received[offset..end] {
            if !*slot {
                *slot = true;
                self.received_count += 1;
            }
        }
        self.last_seen_ms = now_ms;
        true
    }

    fn is_complete(&self) -> bool {
        matches!(self.total_length, Some(total) if self.received_count == total && total >= 8)
    }
}

/// Bounded stateful IPv4 fragment reassembler for UDP datagrams (`protocol == 17`).
#[derive(Debug, Default)]
pub struct Ipv4FragmentAssembler {
    entries: HashMap<Ipv4FragmentKey, Ipv4FragmentAssembly>,
    next_cleanup_ms: u64,
}

impl Ipv4FragmentAssembler {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn pending_count(&self) -> usize {
        self.entries.len()
    }

    /// Feeds a fragmented IPv4 packet. Returns `Some(OwnedUdpDatagram)` once all fragments arrive.
    /// Non-fragmented or non-UDP packets return `None`.
    pub fn try_reassemble_ipv4_udp(
        &mut self,
        raw_ip: &[u8],
        now_ms: u64,
    ) -> Option<OwnedUdpDatagram> {
        self.cleanup_expired(now_ms);

        if raw_ip.len() < 20 || (raw_ip[0] >> 4) != 4 {
            return None;
        }
        let ihl = ((raw_ip[0] & 0x0F) as usize) * 4;
        let total_len = u16::from_be_bytes([raw_ip[2], raw_ip[3]]) as usize;
        if ihl < 20 || total_len < ihl || total_len > raw_ip.len() {
            return None;
        }
        let protocol = raw_ip[9];
        if protocol != 17 {
            return None;
        }

        let identification = u16::from_be_bytes([raw_ip[4], raw_ip[5]]);
        let flags_frag = u16::from_be_bytes([raw_ip[6], raw_ip[7]]);
        let has_more = (flags_frag & 0x2000) != 0;
        let frag_offset = ((flags_frag & 0x1FFF) as usize) * 8;
        if !has_more && frag_offset == 0 {
            return None;
        }

        let frag_payload = &raw_ip[ihl..total_len];
        if frag_payload.is_empty()
            || frag_offset.saturating_add(frag_payload.len()) > MAX_IPV4_PAYLOAD_LEN
        {
            return None;
        }

        let mut src_ip = [0u8; 4];
        let mut dst_ip = [0u8; 4];
        src_ip.copy_from_slice(&raw_ip[12..16]);
        dst_ip.copy_from_slice(&raw_ip[16..20]);
        let key = Ipv4FragmentKey {
            src_ip,
            dst_ip,
            identification,
            protocol,
        };

        if !self.entries.contains_key(&key) {
            self.evict_if_full();
        }

        let entry = self
            .entries
            .entry(key)
            .or_insert_with(|| Ipv4FragmentAssembly::new(now_ms));

        if !entry.insert_fragment(frag_offset, frag_payload, has_more, now_ms) {
            self.entries.remove(&key);
            return None;
        }

        if !entry.is_complete() {
            return None;
        }

        let completed = self.entries.remove(&key)?;
        let total_udp = completed.total_length?;
        let udp = &completed.payload[..total_udp];
        if udp.len() < 8 {
            return None;
        }
        let src_port = u16::from_be_bytes([udp[0], udp[1]]);
        let dst_port = u16::from_be_bytes([udp[2], udp[3]]);
        let udp_len = u16::from_be_bytes([udp[4], udp[5]]) as usize;
        if udp_len < 8 || udp_len > udp.len() {
            return None;
        }

        Some(OwnedUdpDatagram {
            src_port,
            dst_port,
            payload: udp[8..udp_len].to_vec(),
        })
    }

    fn cleanup_expired(&mut self, now_ms: u64) {
        if now_ms < self.next_cleanup_ms {
            return;
        }
        self.next_cleanup_ms = now_ms.saturating_add(IPV4_FRAGMENT_CLEANUP_INTERVAL_MS);
        self.entries
            .retain(|_, v| now_ms.saturating_sub(v.last_seen_ms) < IPV4_FRAGMENT_TIMEOUT_MS);
    }

    fn evict_if_full(&mut self) {
        while self.entries.len() >= MAX_PENDING_IPV4_FRAGMENTS {
            let Some(oldest_key) = self
                .entries
                .iter()
                .min_by_key(|(_, v)| v.last_seen_ms)
                .map(|(k, _)| *k)
            else {
                break;
            };
            self.entries.remove(&oldest_key);
        }
    }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct PhotonFragmentKey {
    peer_id: u16,
    challenge: i32,
    channel_id: u8,
    start_seq: i32,
}

#[derive(Debug, Clone)]
struct PhotonFragmentAssembly {
    total_length: usize,
    fragment_count: usize,
    fragment_offsets: Vec<i32>,
    fragment_lengths: Vec<usize>,
    received_fragments: usize,
    received_bytes: usize,
    data: Vec<u8>,
    last_seen_seq: u64,
}

impl PhotonFragmentAssembly {
    fn new(total_length: usize, fragment_count: usize, seq: u64) -> Self {
        Self {
            total_length,
            fragment_count,
            fragment_offsets: vec![-1; fragment_count],
            fragment_lengths: vec![0; fragment_count],
            received_fragments: 0,
            received_bytes: 0,
            data: vec![0u8; total_length],
            last_seen_seq: seq,
        }
    }

    fn is_compatible(&self, total_length: usize, fragment_count: usize) -> bool {
        self.total_length == total_length && self.fragment_count == fragment_count
    }

    fn try_add_fragment(
        &mut self,
        fragment_number: usize,
        fragment_offset: usize,
        payload: &[u8],
        seq: u64,
    ) -> bool {
        if fragment_number >= self.fragment_count || payload.is_empty() {
            return false;
        }
        let Some(fragment_end) = fragment_offset.checked_add(payload.len()) else {
            return false;
        };
        if fragment_offset > self.total_length || fragment_end > self.total_length {
            return false;
        }

        self.last_seen_seq = seq;

        let existing_offset = self.fragment_offsets[fragment_number];
        if existing_offset >= 0 {
            return existing_offset as usize == fragment_offset
                && self.fragment_lengths[fragment_number] == payload.len();
        }

        if fragment_number > 0 {
            let prev_offset = self.fragment_offsets[fragment_number - 1];
            if prev_offset >= 0
                && (prev_offset as usize).saturating_add(self.fragment_lengths[fragment_number - 1])
                    > fragment_offset
            {
                return false;
            }
        }

        if fragment_number + 1 < self.fragment_count {
            let next_offset = self.fragment_offsets[fragment_number + 1];
            if next_offset >= 0 && fragment_end > (next_offset as usize) {
                return false;
            }
        }

        self.data[fragment_offset..fragment_end].copy_from_slice(payload);
        self.fragment_offsets[fragment_number] = fragment_offset as i32;
        self.fragment_lengths[fragment_number] = payload.len();
        self.received_fragments += 1;
        self.received_bytes += payload.len();
        true
    }

    fn is_complete(&self) -> bool {
        self.received_fragments == self.fragment_count && self.received_bytes == self.total_length
    }
}

/// Stateful Photon UDP packet parser and fragment reassembler.
#[derive(Debug, Default, Clone)]
pub struct PhotonParser {
    pending_segments: BTreeMap<PhotonFragmentKey, PhotonFragmentAssembly>,
    pending_fragment_bytes: usize,
    seq_counter: u64,
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

    /// Parses one raw UDP datagram (which may contain one or more coalesced Photon peer packets)
    /// into zero or more [`PhotonMessage`]s.
    pub fn parse_udp_payload(&mut self, payload: &[u8]) -> Vec<PhotonMessage> {
        let mut out = Vec::new();
        let mut packet_offset = 0usize;
        while packet_offset < payload.len() {
            let remaining = &payload[packet_offset..];
            let Some(packet_len) = try_read_photon_packet_length(remaining) else {
                break;
            };
            if packet_len == 0 {
                break;
            }
            self.parse_single_packet(&remaining[..packet_len], &mut out);
            packet_offset += packet_len;
        }
        out
    }

    fn parse_single_packet(&mut self, payload: &[u8], out: &mut Vec<PhotonMessage>) {
        if payload.len() < PHOTON_HEADER_LEN {
            return;
        }

        let peer_id = u16::from_be_bytes([payload[0], payload[1]]);
        let flags = payload[2];
        let command_count = payload[3] as usize;
        let _timestamp = u32::from_be_bytes([payload[4], payload[5], payload[6], payload[7]]);
        let challenge = i32::from_be_bytes([payload[8], payload[9], payload[10], payload[11]]);

        // Encrypted handshake frames are ignored.
        if flags == FLAG_ENCRYPTED {
            return;
        }

        let mut offset = PHOTON_HEADER_LEN;
        if flags == FLAG_CRC {
            if payload.len() < PHOTON_HEADER_LEN + 4 {
                return;
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
                return;
            }
            offset += 4;
        }

        self.packets_processed += 1;

        for _ in 0..command_count {
            if offset + COMMAND_HEADER_LEN > payload.len() {
                break;
            }
            let command_type = payload[offset];
            let channel_id = payload[offset + 1];
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
                CMD_DISCONNECT => {}
                CMD_SEND_RELIABLE => {
                    if let Some(msg) = Self::decode_reliable_body(body) {
                        out.push(msg);
                    }
                }
                CMD_SEND_UNRELIABLE => {
                    if body.len() >= 4 {
                        if let Some(msg) = Self::decode_reliable_body(&body[4..]) {
                            out.push(msg);
                        }
                    }
                }
                CMD_SEND_FRAGMENT => {
                    if let Some(msg) =
                        self.handle_fragment_body(peer_id, challenge, channel_id, body)
                    {
                        out.push(msg);
                    }
                }
                _ => {}
            }
        }
    }

    fn decode_reliable_body(body: &[u8]) -> Option<PhotonMessage> {
        if body.len() < 2 {
            return None;
        }
        let _signal_byte = body[0]; // 0xF3 or 0xFD
        let message_type = body[1];
        let mut reader = Protocol18Reader::new(&body[2..]);
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

    fn handle_fragment_body(
        &mut self,
        peer_id: u16,
        challenge: i32,
        channel_id: u8,
        body: &[u8],
    ) -> Option<PhotonMessage> {
        if body.len() <= FRAGMENT_HEADER_LEN {
            return None;
        }
        let start_seq = i32::from_be_bytes([body[0], body[1], body[2], body[3]]);
        let fragment_count = i32::from_be_bytes([body[4], body[5], body[6], body[7]]);
        let fragment_number = i32::from_be_bytes([body[8], body[9], body[10], body[11]]);
        let total_length = i32::from_be_bytes([body[12], body[13], body[14], body[15]]);
        let fragment_offset = i32::from_be_bytes([body[16], body[17], body[18], body[19]]);

        if total_length <= 0
            || fragment_count <= 0
            || fragment_number < 0
            || fragment_number >= fragment_count
            || fragment_offset < 0
        {
            return None;
        }
        let total_len = total_length as usize;
        let frag_count = fragment_count as usize;
        let frag_num = fragment_number as usize;
        let frag_off = fragment_offset as usize;
        if total_len > MAX_FRAGMENT_TOTAL_LEN || frag_count > MAX_FRAGMENT_COUNT {
            return None;
        }

        let frag_slice = &body[FRAGMENT_HEADER_LEN..];
        let end_off = frag_off.checked_add(frag_slice.len())?;
        if frag_slice.is_empty() || end_off > total_len {
            return None;
        }

        self.seq_counter = self.seq_counter.saturating_add(1);
        let seq = self.seq_counter;
        let key = PhotonFragmentKey {
            peer_id,
            challenge,
            channel_id,
            start_seq,
        };

        if let Some(existing) = self.pending_segments.get(&key) {
            if !existing.is_compatible(total_len, frag_count) {
                self.remove_fragment_assembly(&key);
            }
        }

        if !self.pending_segments.contains_key(&key) {
            self.trim_fragment_assemblies(total_len);
            self.pending_segments
                .insert(key, PhotonFragmentAssembly::new(total_len, frag_count, seq));
            self.pending_fragment_bytes += total_len;
        }

        let entry = self.pending_segments.get_mut(&key)?;
        if !entry.try_add_fragment(frag_num, frag_off, frag_slice, seq) {
            self.remove_fragment_assembly(&key);
            return None;
        }

        if entry.is_complete() {
            let finished = self.remove_fragment_assembly(&key)?;
            return Self::decode_reliable_body(&finished.data);
        }

        None
    }

    fn remove_fragment_assembly(
        &mut self,
        key: &PhotonFragmentKey,
    ) -> Option<PhotonFragmentAssembly> {
        let removed = self.pending_segments.remove(key)?;
        self.pending_fragment_bytes = self
            .pending_fragment_bytes
            .saturating_sub(removed.total_length);
        Some(removed)
    }

    fn trim_fragment_assemblies(&mut self, required_bytes: usize) {
        while self.pending_segments.len() >= MAX_PENDING_FRAGMENTS
            || self.pending_fragment_bytes
                > MAX_PENDING_FRAGMENT_BYTES.saturating_sub(required_bytes)
        {
            let Some(oldest_key) = self
                .pending_segments
                .iter()
                .min_by_key(|(_, v)| v.last_seen_seq)
                .map(|(k, _)| *k)
            else {
                break;
            };
            self.remove_fragment_assembly(&oldest_key);
        }
    }
}

/// Serializes a [`PhotonMessage`] into its inner reliable Protocol18 payload (`[0xF3, msgType, ...protocol18]`).
pub fn encode_reliable_message_body(msg: &PhotonMessage) -> Vec<u8> {
    let mut body = Vec::new();
    body.push(0xF3);
    match msg {
        PhotonMessage::OperationRequest(req) => {
            body.push(MSG_OPERATION_REQUEST);
            encode_protocol18_operation_request_body(&mut body, req);
        }
        PhotonMessage::OperationResponse(resp) => {
            body.push(MSG_OPERATION_RESPONSE);
            encode_protocol18_operation_response_body(&mut body, resp);
        }
        PhotonMessage::EventData(ev) => {
            body.push(MSG_EVENT_DATA);
            encode_protocol18_event_data_body(&mut body, ev);
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
    build_photon_fragmented_packets_on_channel(msg, chunk_size, start_seq, 0, with_crc)
}

/// Splits a single [`PhotonMessage`] across multiple `SendFragment (8)` Photon UDP packets on `channel_id`.
pub fn build_photon_fragmented_packets_on_channel(
    msg: &PhotonMessage,
    chunk_size: usize,
    start_seq: i32,
    channel_id: u8,
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
        pkt.push(channel_id);
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
        assert!(is_photon_port(5058));

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

    #[test]
    fn processes_coalesced_photon_packets_and_disconnect_followed_by_reliable() {
        let mut p1 = ParameterTable::new();
        p1.insert(253, Protocol16Value::Short(174));
        let m1 = PhotonMessage::OperationResponse(OperationResponsePayload {
            operation_code: 174,
            return_code: 0,
            debug_message: None,
            parameters: p1,
        });
        let mut p2 = ParameterTable::new();
        p2.insert(253, Protocol16Value::Short(176));
        let m2 = PhotonMessage::OperationResponse(OperationResponsePayload {
            operation_code: 176,
            return_code: 0,
            debug_message: None,
            parameters: p2,
        });

        let pkt1 = build_photon_udp_packet(std::slice::from_ref(&m1), false, false, 1);
        let pkt2 = build_photon_udp_packet(std::slice::from_ref(&m2), true, false, 2);
        let mut coalesced = Vec::with_capacity(pkt1.len() + pkt2.len());
        coalesced.extend_from_slice(&pkt1);
        coalesced.extend_from_slice(&pkt2);

        assert_eq!(try_read_photon_packet_length(&coalesced), Some(pkt1.len()));
        assert_eq!(try_read_photon_packet_length(&pkt1[..pkt1.len() - 1]), None);

        let mut parser = PhotonParser::new();
        let decoded = parser.parse_udp_payload(&coalesced);
        assert_eq!(decoded, vec![m1.clone(), m2]);
        assert_eq!(parser.packets_processed, 2);

        // Disconnect command followed by SendReliable in same Photon packet must NOT abort loop
        let rel_body = encode_reliable_message_body(&m1);
        let rel_len = (COMMAND_HEADER_LEN + rel_body.len()) as i32;
        let mut mixed = Vec::new();
        mixed.extend_from_slice(&1u16.to_be_bytes());
        mixed.push(0); // flags
        mixed.push(2); // 2 commands
        mixed.extend_from_slice(&1000u32.to_be_bytes());
        mixed.extend_from_slice(&123i32.to_be_bytes());
        // Command 1: Disconnect (12 bytes)
        mixed.push(CMD_DISCONNECT);
        mixed.extend_from_slice(&[0, 0, 0]);
        mixed.extend_from_slice(&(COMMAND_HEADER_LEN as i32).to_be_bytes());
        mixed.extend_from_slice(&1i32.to_be_bytes());
        // Command 2: SendReliable
        mixed.push(CMD_SEND_RELIABLE);
        mixed.extend_from_slice(&[0, 1, 0]);
        mixed.extend_from_slice(&rel_len.to_be_bytes());
        mixed.extend_from_slice(&2i32.to_be_bytes());
        mixed.extend_from_slice(&rel_body);

        let decoded_mixed = parser.parse_udp_payload(&mixed);
        assert_eq!(decoded_mixed, vec![m1]);
    }

    #[test]
    fn handles_out_of_order_duplicate_missing_and_multi_channel_fragments() {
        // 1. Large out-of-order + duplicate fragment reassembly
        let expected_data: Vec<u8> = (0..40_000).map(|i| (i % 251) as u8).collect();
        let mut params = ParameterTable::new();
        params.insert(1, Protocol16Value::ByteArray(expected_data.clone()));
        params.insert(253, Protocol16Value::Short(174));
        let large_msg = PhotonMessage::OperationResponse(OperationResponsePayload {
            operation_code: 174,
            return_code: 0,
            debug_message: None,
            parameters: params,
        });
        let frags = build_photon_fragmented_packets_on_channel(&large_msg, 900, 4000, 1, false);
        assert!(frags.len() > 20);

        let mut parser = PhotonParser::new();
        // Send last fragment twice, then reverse remaining
        assert!(parser.parse_udp_payload(frags.last().unwrap()).is_empty());
        assert!(parser.parse_udp_payload(frags.last().unwrap()).is_empty());
        let mut emitted = Vec::new();
        for frag in frags[..frags.len() - 1].iter().rev() {
            emitted.extend(parser.parse_udp_payload(frag));
        }
        assert_eq!(emitted, vec![large_msg]);
        assert_eq!(parser.pending_fragment_count(), 0);

        // 2. Interleaved same start_seq on different channels stay independent
        let mut p1 = ParameterTable::new();
        p1.insert(1, Protocol16Value::ByteArray(vec![1u8; 4_000]));
        p1.insert(253, Protocol16Value::Short(174));
        let msg_ch1 = PhotonMessage::OperationResponse(OperationResponsePayload {
            operation_code: 174,
            return_code: 0,
            debug_message: None,
            parameters: p1,
        });
        let mut p2 = ParameterTable::new();
        p2.insert(1, Protocol16Value::ByteArray(vec![2u8; 4_000]));
        p2.insert(253, Protocol16Value::Short(176));
        let msg_ch2 = PhotonMessage::OperationResponse(OperationResponsePayload {
            operation_code: 176,
            return_code: 0,
            debug_message: None,
            parameters: p2,
        });
        let f1 = build_photon_fragmented_packets_on_channel(&msg_ch1, 700, 5000, 1, false);
        let f2 = build_photon_fragmented_packets_on_channel(&msg_ch2, 700, 5000, 2, false);
        assert_eq!(f1.len(), f2.len());
        let mut channel_out = Vec::new();
        for (a, b) in f1.iter().zip(f2.iter()) {
            channel_out.extend(parser.parse_udp_payload(a));
            channel_out.extend(parser.parse_udp_payload(b));
        }
        assert_eq!(channel_out, vec![msg_ch1, msg_ch2]);

        // 3. Missing one fragment does not emit partial message
        let mut p3 = ParameterTable::new();
        p3.insert(1, Protocol16Value::ByteArray(vec![0u8; 5_000]));
        let msg_missing = PhotonMessage::OperationResponse(OperationResponsePayload {
            operation_code: 174,
            return_code: 0,
            debug_message: None,
            parameters: p3,
        });
        let fm = build_photon_fragmented_packets_on_channel(&msg_missing, 700, 6000, 1, false);
        for (idx, frag) in fm.iter().enumerate() {
            if idx != 2 {
                assert!(parser.parse_udp_payload(frag).is_empty());
            }
        }
        assert_eq!(parser.pending_fragment_count(), 1);
    }

    #[test]
    fn reassembles_fragmented_ipv4_udp_and_parses_ipv6_udp() {
        let mut params = ParameterTable::new();
        params.insert(0, Protocol16Value::Long(555));
        params.insert(252, Protocol16Value::Short(81));
        let msg = PhotonMessage::EventData(EventDataPayload {
            code: 1,
            parameters: params,
        });
        let udp_payload = build_photon_udp_packet(std::slice::from_ref(&msg), true, false, 1);

        // Build full IPv4 UDP bytes after IP header (8 + payload)
        let udp_len = (8 + udp_payload.len()) as u16;
        let mut udp_segment = Vec::with_capacity(udp_len as usize);
        udp_segment.extend_from_slice(&5056u16.to_be_bytes());
        udp_segment.extend_from_slice(&51234u16.to_be_bytes());
        udp_segment.extend_from_slice(&udp_len.to_be_bytes());
        udp_segment.extend_from_slice(&0u16.to_be_bytes());
        udp_segment.extend_from_slice(&udp_payload);

        // Split into 2 IPv4 fragments (16 bytes first chunk, remainder second chunk)
        let split = 16usize;
        let build_frag = |offset_bytes: usize, chunk: &[u8], mf: bool| -> Vec<u8> {
            let total_len = (20 + chunk.len()) as u16;
            let flags_frag = ((offset_bytes / 8) as u16) | if mf { 0x2000 } else { 0 };
            let mut raw = Vec::with_capacity(total_len as usize);
            raw.push(0x45);
            raw.push(0x00);
            raw.extend_from_slice(&total_len.to_be_bytes());
            raw.extend_from_slice(&0xABCDu16.to_be_bytes()); // id
            raw.extend_from_slice(&flags_frag.to_be_bytes());
            raw.push(64);
            raw.push(17); // UDP
            raw.extend_from_slice(&0u16.to_be_bytes());
            raw.extend_from_slice(&[193, 169, 238, 10]);
            raw.extend_from_slice(&[192, 168, 1, 20]);
            raw.extend_from_slice(chunk);
            raw
        };

        let frag1 = build_frag(0, &udp_segment[..split], true);
        let frag2 = build_frag(split, &udp_segment[split..], false);

        // Non-assembled fragments must NOT be accepted directly as unfragmented UDP
        assert!(extract_udp_from_ip_packet(&frag1).is_none());
        assert!(extract_udp_from_ip_packet(&frag2).is_none());

        let mut ip_asm = Ipv4FragmentAssembler::new();
        // Out-of-order: second fragment arrives first
        assert!(ip_asm.try_reassemble_ipv4_udp(&frag2, 10_000).is_none());
        assert_eq!(ip_asm.pending_count(), 1);
        let reassembled = ip_asm.try_reassemble_ipv4_udp(&frag1, 10_050).unwrap();
        assert_eq!(ip_asm.pending_count(), 0);
        assert_eq!(reassembled.src_port, 5056);
        assert_eq!(reassembled.payload, udp_payload);

        let mut parser = PhotonParser::new();
        assert_eq!(
            parser.parse_udp_payload(&reassembled.payload),
            vec![msg.clone()]
        );

        // Verify IPv6 UDP extraction trims padding beyond IPv6 payload_len
        let mut ip6 = Vec::new();
        ip6.extend_from_slice(&[0x60, 0x00, 0x00, 0x00]); // version 6
        ip6.extend_from_slice(&udp_len.to_be_bytes()); // payload_len = udp_len
        ip6.push(17); // next header = UDP
        ip6.push(64); // hop limit
        ip6.extend_from_slice(&[0x20; 16]); // src
        ip6.extend_from_slice(&[0x2a; 16]); // dst
        ip6.extend_from_slice(&udp_segment);
        ip6.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]); // trailing FCS/padding

        let slice6 = extract_udp_from_ip_packet(&ip6).unwrap();
        assert_eq!(slice6.src_port, 5056);
        assert_eq!(slice6.payload, &udp_payload[..]);
    }
}
