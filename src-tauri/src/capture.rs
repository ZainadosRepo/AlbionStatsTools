//! Passive Photon UDP network capture on ports `5055-5060` and `4530-4540`:
//! - Windows: native Npcap NDIS capture first (Ethernet/VLAN/PPPoE/IPv4/IPv6 + IPv4 fragments),
//!   falling back to multi-interface Winsock2 raw sockets (IPv4 `SIO_RCVALL` + IPv6 `IPPROTO_UDP`)
//! - macOS: passive BPF capture via `/usr/sbin/tcpdump` when `/dev/bpf*` access is available
//! - All platforms: local loopback relay listener on `127.0.0.1:50560`.

use albion_core::engine::AnalyzerEngine;
use albion_core::photon::{
    extract_udp_from_ip_packet, is_photon_port, looks_like_photon, Ipv4FragmentAssembler,
    OwnedUdpDatagram, UdpDatagramSlice,
};
use serde::{Deserialize, Serialize};
#[cfg(any(target_os = "windows", test))]
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::net::{SocketAddr, UdpSocket};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureStatus {
    pub mode: String,
    pub bind_address: String,
    pub detail: String,
    pub raw_datagrams_seen: u64,
    pub needs_elevation: bool,
}

impl Default for CaptureStatus {
    fn default() -> Self {
        Self {
            mode: "initializing".into(),
            bind_address: "0.0.0.0:5056".into(),
            detail: "Initialisation de l'écoute passive Photon UDP (ports 5055-5060 / 4530-4540)…"
                .into(),
            raw_datagrams_seen: 0,
            needs_elevation: false,
        }
    }
}

pub fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(target_os = "windows")]
fn enumerate_windows_adapter_ips(out: &mut Vec<Ipv4Addr>) {
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct MibIpAddrRow {
        dw_addr: u32,
        dw_index: u32,
        dw_mask: u32,
        dw_bcast_addr: u32,
        dw_reasm_size: u32,
        unused1: u16,
        w_type: u16,
    }

    #[link(name = "iphlpapi")]
    extern "system" {
        fn GetIpAddrTable(p_ip_addr_table: *mut u8, pdw_size: *mut u32, b_order: i32) -> u32;
    }

    let mut size: u32 = 0;
    unsafe {
        let _ = GetIpAddrTable(core::ptr::null_mut(), &mut size, 0);
        if size > 0 && size <= 65_536 {
            let mut buf = vec![0u8; size as usize];
            if GetIpAddrTable(buf.as_mut_ptr(), &mut size, 0) == 0 && buf.len() >= 4 {
                let num_entries = u32::from_ne_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
                let row_size = core::mem::size_of::<MibIpAddrRow>();
                for i in 0..num_entries {
                    let offset = 4 + i * row_size;
                    if offset + row_size > buf.len() {
                        break;
                    }
                    let row = *(buf.as_ptr().add(offset) as *const MibIpAddrRow);
                    let ip = Ipv4Addr::from(u32::from_be(row.dw_addr));
                    if !ip.is_loopback()
                        && !ip.is_unspecified()
                        && !ip.is_link_local()
                        && !out.contains(&ip)
                    {
                        out.push(ip);
                    }
                }
            }
        }
    }
}

/// Discovers all candidate outbound IPv4 addresses used to reach Albion Online servers
/// (Europe, Americas, Asia) as well as all active physical, Wi-Fi, and VPN network adapters.
#[cfg(any(target_os = "windows", test))]
pub fn detect_candidate_ipv4s() -> Vec<Ipv4Addr> {
    const ROUTE_PROBES: [&str; 6] = [
        "193.169.238.1:5056", // Europe (Amsterdam)
        "193.169.239.1:5056",
        "5.45.187.1:5056",  // Americas (Washington)
        "5.188.125.1:5056", // Asia (Singapore)
        "1.1.1.1:53",
        "8.8.8.8:53",
    ];
    let mut out = Vec::new();
    #[cfg(target_os = "windows")]
    {
        enumerate_windows_adapter_ips(&mut out);
    }
    for target in ROUTE_PROBES {
        let Ok(sock) = UdpSocket::bind("0.0.0.0:0") else {
            continue;
        };
        if sock.connect(target).is_err() {
            continue;
        }
        if let Ok(addr) = sock.local_addr() {
            if let IpAddr::V4(ip) = addr.ip() {
                if !ip.is_loopback()
                    && !ip.is_unspecified()
                    && !ip.is_link_local()
                    && !out.contains(&ip)
                {
                    out.push(ip);
                }
            }
        }
    }
    out
}

/// Discovers candidate global/ULA outbound IPv6 addresses reachable for Internet UDP traffic.
#[cfg(any(target_os = "windows", test))]
pub fn detect_candidate_ipv6s() -> Vec<Ipv6Addr> {
    const ROUTE_PROBES_V6: [&str; 2] = [
        "[2606:4700:4700::1111]:53", // Cloudflare IPv6
        "[2001:4860:4860::8888]:53", // Google IPv6
    ];
    let mut out = Vec::new();
    for target in ROUTE_PROBES_V6 {
        let Ok(sock) = UdpSocket::bind("[::]:0") else {
            continue;
        };
        if sock.connect(target).is_err() {
            continue;
        }
        if let Ok(addr) = sock.local_addr() {
            if let IpAddr::V6(ip) = addr.ip() {
                let is_link_local = (ip.segments()[0] & 0xffc0) == 0xfe80;
                if !ip.is_loopback()
                    && !ip.is_unspecified()
                    && !ip.is_multicast()
                    && !is_link_local
                    && !out.contains(&ip)
                {
                    out.push(ip);
                }
            }
        }
    }
    out
}

/// Parses a single `tcpdump -xx` hex line (`"  0x0000:  4500 0024 ..."`), appending raw bytes.
#[cfg(any(target_os = "macos", test))]
pub fn parse_tcpdump_hex_line(line: &str, out: &mut Vec<u8>) -> bool {
    let trimmed = line.trim_start();
    if !trimmed.starts_with("0x") {
        return false;
    }
    let Some((_, hex_part)) = trimmed.split_once(':') else {
        return false;
    };
    for token in hex_part.split_whitespace() {
        if token.len() == 4 && token.bytes().all(|b| b.is_ascii_hexdigit()) {
            if let (Ok(hi), Ok(lo)) = (
                u8::from_str_radix(&token[0..2], 16),
                u8::from_str_radix(&token[2..4], 16),
            ) {
                out.push(hi);
                out.push(lo);
            }
        } else if token.len() == 2 && token.bytes().all(|b| b.is_ascii_hexdigit()) {
            if let Ok(b) = u8::from_str_radix(token, 16) {
                out.push(b);
            }
        } else {
            break;
        }
    }
    true
}

/// Returns true if a Winsock error code is a transient condition that should not
/// terminate the raw socket capture loop.
#[inline]
pub fn is_transient_winsock_error(err: i32) -> bool {
    // 10054: WSAECONNRESET (ICMP port unreachable delivered to UDP raw socket)
    // 10040: WSAEMSGSIZE (datagram exceeded receive buffer)
    // 10035: WSAEWOULDBLOCK (non-blocking call would block)
    // 10060: WSAETIMEDOUT (socket timed out)
    // 10004: WSAEINTR (interrupted system call)
    matches!(err, 10054 | 10040 | 10035 | 10060 | 10004)
}

/// Scans a link-layer frame (Ethernet / BSD loopback / PKTAP / Npcap) for an IPv4/IPv6 Photon UDP packet.
pub fn find_ipv4_udp_in_frame(frame: &[u8]) -> Option<UdpDatagramSlice<'_>> {
    let max_offset = frame.len().min(64);
    for offset in 0..max_offset {
        let candidate = &frame[offset..];
        let Some(udp) = extract_udp_from_ip_packet(candidate) else {
            continue;
        };
        if is_photon_port(udp.src_port)
            || is_photon_port(udp.dst_port)
            || looks_like_photon(udp.payload)
        {
            return Some(udp);
        }
    }
    None
}

/// Extracts a Photon UDP datagram from a headerless UDP segment `[UDP header (8B) | payload]`,
/// as delivered by Winsock2 `AF_INET6` `SOCK_RAW` `IPPROTO_UDP` sockets on Windows.
/// Requires at least one endpoint port to match a Photon UDP port to avoid false positives.
#[cfg(any(target_os = "windows", test))]
pub fn extract_udp_segment_for_photon(segment: &[u8]) -> Option<UdpDatagramSlice<'_>> {
    if segment.len() < 8 {
        return None;
    }
    let src_port = u16::from_be_bytes([segment[0], segment[1]]);
    let dst_port = u16::from_be_bytes([segment[2], segment[3]]);
    if !is_photon_port(src_port) && !is_photon_port(dst_port) {
        return None;
    }
    let udp_len = u16::from_be_bytes([segment[4], segment[5]]) as usize;
    if udp_len < 8 || udp_len > segment.len() {
        return None;
    }
    Some(UdpDatagramSlice {
        src_port,
        dst_port,
        payload: &segment[8..udp_len],
    })
}

#[inline]
fn matches_photon_dgram(dgram: &OwnedUdpDatagram) -> bool {
    is_photon_port(dgram.src_port)
        || is_photon_port(dgram.dst_port)
        || looks_like_photon(&dgram.payload)
}

/// Extracts a Photon UDP datagram from a link-layer capture frame given its `pcap_datalink` type,
/// handling Ethernet II (`DLT_EN10MB`, including up to 2 `802.1Q`/`802.1ad` VLAN tags and PPPoE),
/// raw IP (`DLT_RAW`), BSD loopback (`DLT_NULL`/`DLT_LOOP`), Linux SLL/SLL2, and IPv4 fragment reassembly.
pub fn extract_photon_udp_from_datalink(
    datalink: Option<i32>,
    frame: &[u8],
    ipv4_fragments: &mut Ipv4FragmentAssembler,
    now_ms: u64,
) -> Option<OwnedUdpDatagram> {
    let ip_slice = match datalink {
        // DLT_EN10MB (Ethernet II + optional VLAN / PPPoE)
        Some(1) => {
            if frame.len() < 14 {
                return None;
            }
            let mut offset = 14usize;
            let mut ethertype = u16::from_be_bytes([frame[12], frame[13]]);
            for _ in 0..2 {
                if matches!(ethertype, 0x8100 | 0x88A8 | 0x9100) {
                    if frame.len() < offset + 4 {
                        return None;
                    }
                    ethertype = u16::from_be_bytes([frame[offset + 2], frame[offset + 3]]);
                    offset += 4;
                } else {
                    break;
                }
            }
            if ethertype == 0x8864 {
                if frame.len() < offset + 8 {
                    return None;
                }
                let ppp_proto = u16::from_be_bytes([frame[offset + 6], frame[offset + 7]]);
                offset += 8;
                ethertype = match ppp_proto {
                    0x0021 => 0x0800,
                    0x0057 => 0x86DD,
                    _ => return None,
                };
            }
            if !matches!(ethertype, 0x0800 | 0x86DD) {
                return None;
            }
            Some(&frame[offset..])
        }
        // DLT_RAW
        Some(12 | 14 | 101) => Some(frame),
        // DLT_NULL / DLT_LOOP
        Some(0 | 108) => {
            if frame.len() <= 4 {
                return None;
            }
            Some(&frame[4..])
        }
        // DLT_LINUX_SLL
        Some(113) => {
            if frame.len() < 16 {
                return None;
            }
            let proto = u16::from_be_bytes([frame[14], frame[15]]);
            if !matches!(proto, 0x0800 | 0x86DD) {
                return None;
            }
            Some(&frame[16..])
        }
        // DLT_LINUX_SLL2
        Some(276) => {
            if frame.len() < 20 {
                return None;
            }
            let proto = u16::from_be_bytes([frame[0], frame[1]]);
            if !matches!(proto, 0x0800 | 0x86DD) {
                return None;
            }
            Some(&frame[20..])
        }
        _ => None,
    };

    if let Some(ip_packet) = ip_slice {
        if let Some(udp) = extract_udp_from_ip_packet(ip_packet) {
            if is_photon_port(udp.src_port)
                || is_photon_port(udp.dst_port)
                || looks_like_photon(udp.payload)
            {
                return Some(OwnedUdpDatagram {
                    src_port: udp.src_port,
                    dst_port: udp.dst_port,
                    payload: udp.payload.to_vec(),
                });
            }
            return None;
        }
        if let Some(dgram) = ipv4_fragments.try_reassemble_ipv4_udp(ip_packet, now_ms) {
            if matches_photon_dgram(&dgram) {
                return Some(dgram);
            }
        }
        return None;
    }

    if let Some(udp) = find_ipv4_udp_in_frame(frame) {
        return Some(OwnedUdpDatagram {
            src_port: udp.src_port,
            dst_port: udp.dst_port,
            payload: udp.payload.to_vec(),
        });
    }

    let max_offset = frame.len().min(64);
    for offset in 0..max_offset {
        let candidate = &frame[offset..];
        if candidate.len() < 20 {
            break;
        }
        if (candidate[0] >> 4) != 4 || candidate[9] != 17 {
            continue;
        }
        let ihl = ((candidate[0] & 0x0F) as usize) * 4;
        let total_len = u16::from_be_bytes([candidate[2], candidate[3]]) as usize;
        let flags_frag = u16::from_be_bytes([candidate[6], candidate[7]]);
        if ihl < 20 || total_len <= ihl || total_len > candidate.len() || (flags_frag & 0x3FFF) == 0
        {
            continue;
        }
        if let Some(dgram) = ipv4_fragments.try_reassemble_ipv4_udp(candidate, now_ms) {
            if matches_photon_dgram(&dgram) {
                return Some(dgram);
            }
        }
    }
    None
}

/// Relaunches the current executable on Windows via UAC (`ShellExecuteW` with `"runas"`).
pub fn relaunch_as_admin() -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::ffi::OsStrExt;

        #[link(name = "shell32")]
        extern "system" {
            fn ShellExecuteW(
                hwnd: usize,
                lpOperation: *const u16,
                lpFile: *const u16,
                lpParameters: *const u16,
                lpDirectory: *const u16,
                nShowCmd: i32,
            ) -> isize;
        }

        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let verb: Vec<u16> = "runas\0".encode_utf16().collect();
        let mut file: Vec<u16> = exe.as_os_str().encode_wide().collect();
        file.push(0);

        // SAFETY: Null-terminated UTF-16 buffers passed to ShellExecuteW.
        let res = unsafe {
            ShellExecuteW(
                0,
                verb.as_ptr(),
                file.as_ptr(),
                core::ptr::null::<u16>(),
                core::ptr::null::<u16>(),
                1,
            )
        };
        if res <= 32 {
            return Err(format!(
                "Élévation Administrateur annulée ou refusée (code ShellExecute {res})"
            ));
        }
        Ok(())
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err("L'élévation UAC automatique n'est disponible que sous Windows.".into())
    }
}

/// Starts background passive network listeners feeding `engine`.
pub fn start_background_capture(
    engine: Arc<Mutex<AnalyzerEngine>>,
    status: Arc<Mutex<CaptureStatus>>,
) {
    // 1. Local UDP relay socket (127.0.0.1:50560) for non-elevated packet injection / testing
    let engine_relay = Arc::clone(&engine);
    let status_relay = Arc::clone(&status);
    std::thread::Builder::new()
        .name("photon-udp-relay".into())
        .spawn(move || {
            let Ok(sock) = UdpSocket::bind(SocketAddr::from(([127, 0, 0, 1], 50560))) else {
                return;
            };
            let mut buf = vec![0u8; 65_535];
            while let Ok((len, _peer)) = sock.recv_from(&mut buf) {
                let slice = &buf[..len];
                if looks_like_photon(slice) {
                    if let Ok(mut st) = status_relay.lock() {
                        st.raw_datagrams_seen += 1;
                    }
                    if let Ok(mut eng) = engine_relay.lock() {
                        eng.ingest_udp_payload(slice, now_unix_ms());
                    }
                }
            }
        })
        .ok();

    // 2. Windows multi-interface raw socket (`SIO_RCVALL`) supervisor
    #[cfg(target_os = "windows")]
    {
        std::thread::Builder::new()
            .name("photon-raw-supervisor".into())
            .spawn(move || windows_capture_supervisor(engine, status))
            .ok();
    }

    // 3. macOS passive BPF capture via `/usr/sbin/tcpdump` (falls back cleanly to standby)
    #[cfg(target_os = "macos")]
    {
        std::thread::Builder::new()
            .name("photon-macos-bpf".into())
            .spawn(move || macos_bpf_capture_loop(engine, status))
            .ok();
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        if let Ok(mut st) = status.lock() {
            st.mode = "standby".into();
            st.bind_address = "127.0.0.1:50560".into();
            st.detail = "Mode veille actif (relais UDP local 127.0.0.1:50560)".into();
            st.needs_elevation = false;
        }
    }
}

#[cfg(target_os = "macos")]
fn macos_bpf_capture_loop(engine: Arc<Mutex<AnalyzerEngine>>, status: Arc<Mutex<CaptureStatus>>) {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};

    if let Ok(mut st) = status.lock() {
        st.mode = "standby".into();
        st.bind_address = "127.0.0.1:50560".into();
        st.detail =
            "Mode veille macOS (relais UDP 127.0.0.1:50560 + vérification BPF Photon…)".into();
        st.needs_elevation = false;
    }

    let Ok(mut child) = Command::new("/usr/sbin/tcpdump")
        .args([
            "-i",
            "any",
            "-n",
            "-l",
            "-q",
            "-xx",
            "-s",
            "0",
            "udp and (portrange 5055-5060 or portrange 4530-4540)",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return;
    };

    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        return;
    };

    let reader = BufReader::new(stdout);
    let mut frame = Vec::with_capacity(2048);
    let mut ipv4_fragments = Ipv4FragmentAssembler::new();
    let mut promoted_live = false;

    for line in reader.lines().map_while(Result::ok) {
        if !promoted_live {
            promoted_live = true;
            if let Ok(mut st) = status.lock() {
                st.mode = "rawSocket".into();
                st.bind_address = "any:5056".into();
                st.detail = "Capture passive macOS BPF active (UDP 5055-5060/4530-4540)".into();
            }
        }
        if parse_tcpdump_hex_line(&line, &mut frame) {
            continue;
        }
        if !frame.is_empty() {
            let now_ms = now_unix_ms();
            if let Some(udp) =
                extract_photon_udp_from_datalink(None, &frame, &mut ipv4_fragments, now_ms)
            {
                if let Ok(mut st) = status.lock() {
                    st.raw_datagrams_seen += 1;
                }
                if let Ok(mut eng) = engine.lock() {
                    eng.ingest_udp_payload(&udp.payload, now_ms);
                }
            }
            frame.clear();
        }
    }

    let _ = child.wait();
    if let Ok(mut st) = status.lock() {
        st.mode = "standby".into();
        st.bind_address = "127.0.0.1:50560".into();
        st.detail = "Mode veille macOS (relais UDP 127.0.0.1:50560 — installez ChmodBPF pour la capture directe)".into();
    }
}

#[cfg(target_os = "windows")]
mod npcap {
    use std::sync::Arc;

    #[repr(C)]
    #[allow(dead_code)]
    pub struct PcapIf {
        pub next: *mut PcapIf,
        pub name: *const i8,
        pub description: *const i8,
        pub addresses: *mut core::ffi::c_void,
        pub flags: u32,
    }

    #[repr(C)]
    #[allow(dead_code)]
    pub struct BpfProgram {
        pub bf_len: u32,
        pub bf_insns: *mut core::ffi::c_void,
    }

    #[repr(C)]
    #[allow(dead_code)]
    pub struct PcapPkthdr {
        pub ts_sec: i32,
        pub ts_usec: i32,
        pub caplen: u32,
        pub len: u32,
    }

    pub const PCAP_IF_LOOPBACK: u32 = 0x0000_0001;
    pub const PCAP_IF_UP: u32 = 0x0000_0002;

    #[repr(transparent)]
    pub struct SendPcapHandle(pub *mut core::ffi::c_void);

    // SAFETY: Each open pcap_t handle is moved into and used exclusively by one worker thread.
    unsafe impl Send for SendPcapHandle {}

    pub type FnFindAllDevs = unsafe extern "system" fn(*mut *mut PcapIf, *mut i8) -> i32;
    pub type FnFreeAllDevs = unsafe extern "system" fn(*mut PcapIf);
    pub type FnOpenLive =
        unsafe extern "system" fn(*const i8, i32, i32, i32, *mut i8) -> *mut core::ffi::c_void;
    pub type FnCompile = unsafe extern "system" fn(
        *mut core::ffi::c_void,
        *mut BpfProgram,
        *const i8,
        i32,
        u32,
    ) -> i32;
    pub type FnSetFilter =
        unsafe extern "system" fn(*mut core::ffi::c_void, *mut BpfProgram) -> i32;
    pub type FnFreeCode = unsafe extern "system" fn(*mut BpfProgram);
    pub type FnDatalink = unsafe extern "system" fn(*mut core::ffi::c_void) -> i32;
    pub type FnNextEx = unsafe extern "system" fn(
        *mut core::ffi::c_void,
        *mut *mut PcapPkthdr,
        *mut *const u8,
    ) -> i32;
    pub type FnClose = unsafe extern "system" fn(*mut core::ffi::c_void);

    #[allow(dead_code)]
    pub struct NpcapLib {
        pub _handle: usize,
        pub pcap_findalldevs: FnFindAllDevs,
        pub pcap_freealldevs: FnFreeAllDevs,
        pub pcap_open_live: FnOpenLive,
        pub pcap_compile: FnCompile,
        pub pcap_setfilter: FnSetFilter,
        pub pcap_freecode: FnFreeCode,
        pub pcap_datalink: FnDatalink,
        pub pcap_next_ex: FnNextEx,
        pub pcap_close: FnClose,
    }

    unsafe impl Send for NpcapLib {}
    unsafe impl Sync for NpcapLib {}

    #[link(name = "kernel32")]
    extern "system" {
        fn LoadLibraryA(lpLibFileName: *const i8) -> usize;
        fn GetProcAddress(hModule: usize, lpProcName: *const i8) -> *const core::ffi::c_void;
        fn SetDllDirectoryA(lpPathName: *const i8) -> i32;
    }

    pub fn try_load_npcap() -> Option<Arc<NpcapLib>> {
        unsafe {
            let _ = SetDllDirectoryA(b"C:\\Windows\\System32\\Npcap\0".as_ptr() as *const i8);
        }

        let candidates = [
            b"wpcap.dll\0".as_ptr() as *const i8,
            b"C:\\Windows\\System32\\Npcap\\wpcap.dll\0".as_ptr() as *const i8,
            b"C:\\Windows\\SysWOW64\\Npcap\\wpcap.dll\0".as_ptr() as *const i8,
        ];

        let mut h = 0;
        for path in candidates {
            h = unsafe { LoadLibraryA(path) };
            if h != 0 {
                break;
            }
        }

        if h == 0 {
            return None;
        }

        unsafe {
            macro_rules! resolve {
                ($sym:literal, $ty:ty) => {{
                    let ptr = GetProcAddress(h, concat!($sym, "\0").as_ptr() as *const i8);
                    if ptr.is_null() {
                        return None;
                    }
                    core::mem::transmute::<*const core::ffi::c_void, $ty>(ptr)
                }};
            }

            let pcap_findalldevs = resolve!("pcap_findalldevs", FnFindAllDevs);
            let pcap_freealldevs = resolve!("pcap_freealldevs", FnFreeAllDevs);
            let pcap_open_live = resolve!("pcap_open_live", FnOpenLive);
            let pcap_compile = resolve!("pcap_compile", FnCompile);
            let pcap_setfilter = resolve!("pcap_setfilter", FnSetFilter);
            let pcap_freecode = resolve!("pcap_freecode", FnFreeCode);
            let pcap_datalink = resolve!("pcap_datalink", FnDatalink);
            let pcap_next_ex = resolve!("pcap_next_ex", FnNextEx);
            let pcap_close = resolve!("pcap_close", FnClose);

            Some(Arc::new(NpcapLib {
                _handle: h,
                pcap_findalldevs,
                pcap_freealldevs,
                pcap_open_live,
                pcap_compile,
                pcap_setfilter,
                pcap_freecode,
                pcap_datalink,
                pcap_next_ex,
                pcap_close,
            }))
        }
    }
}

#[cfg(target_os = "windows")]
fn windows_capture_supervisor(
    engine: Arc<Mutex<AnalyzerEngine>>,
    status: Arc<Mutex<CaptureStatus>>,
) {
    // 1. Try native Npcap (NDIS kernel capture driver) first.
    // Npcap bypasses Windows Filtering Platform (WFP) and captures inbound UDP without raw socket restrictions.
    if let Some(lib) = npcap::try_load_npcap() {
        if start_npcap_capture(Arc::clone(&lib), Arc::clone(&engine), Arc::clone(&status)) {
            // Npcap capture threads are running across all active physical/virtual network adapters.
            // Supervisor remains idle to avoid duplicate packets.
            loop {
                std::thread::sleep(std::time::Duration::from_secs(60));
            }
        }
    }

    // 2. Fall back to multi-interface raw sockets (`SIO_RCVALL`) with transient error suppression.
    windows_raw_supervisor(engine, status);
}

#[cfg(target_os = "windows")]
fn start_npcap_capture(
    lib: Arc<npcap::NpcapLib>,
    engine: Arc<Mutex<AnalyzerEngine>>,
    status: Arc<Mutex<CaptureStatus>>,
) -> bool {
    use npcap::{BpfProgram, PcapIf, SendPcapHandle, PCAP_IF_LOOPBACK, PCAP_IF_UP};
    use std::ffi::CStr;

    let mut alldevs: *mut PcapIf = core::ptr::null_mut();
    let mut errbuf = [0i8; 256];
    let res = unsafe { (lib.pcap_findalldevs)(&mut alldevs, errbuf.as_mut_ptr()) };

    if res != 0 || alldevs.is_null() {
        let err_str = unsafe { CStr::from_ptr(errbuf.as_ptr()) }
            .to_string_lossy()
            .into_owned();
        if let Ok(mut st) = status.lock() {
            st.detail = format!(
                "Npcap détecté mais énumération impossible ({err_str}) — passage en mode raw socket"
            );
        }
        return false;
    }

    let bpf_primary = b"((ip and ((udp and (portrange 5055-5060 or portrange 4530-4540)) or (ip[6:2] & 0x3fff != 0))) or (ip6 and (udp and (portrange 5055-5060 or portrange 4530-4540))))\0";
    let bpf_fallback = b"udp and (port 5055 or port 5056 or port 5058 or port 4535)\0";
    let mut opened_count = 0usize;

    // Pass 1 opens adapters reporting PCAP_IF_UP; Pass 2 opens non-loopback adapters if none reported UP.
    for want_up in [true, false] {
        if !want_up && opened_count > 0 {
            break;
        }
        let mut curr = alldevs;
        while !curr.is_null() {
            let dev = unsafe { &*curr };
            let is_non_loopback = !dev.name.is_null() && (dev.flags & PCAP_IF_LOOPBACK == 0);
            let is_up = (dev.flags & PCAP_IF_UP) != 0;
            if is_non_loopback && is_up == want_up {
                let mut open_err = [0i8; 256];
                let raw_handle =
                    unsafe { (lib.pcap_open_live)(dev.name, 65_535, 1, 20, open_err.as_mut_ptr()) };
                if !raw_handle.is_null() {
                    let apply_filter = |expr: &[u8]| -> bool {
                        let mut bpf = BpfProgram {
                            bf_len: 0,
                            bf_insns: core::ptr::null_mut(),
                        };
                        let comp_res = unsafe {
                            (lib.pcap_compile)(
                                raw_handle,
                                &mut bpf,
                                expr.as_ptr() as *const i8,
                                1,
                                0xFFFF_FFFF,
                            )
                        };
                        if comp_res == 0 {
                            let set_res = unsafe { (lib.pcap_setfilter)(raw_handle, &mut bpf) };
                            unsafe { (lib.pcap_freecode)(&mut bpf) };
                            set_res == 0
                        } else {
                            false
                        }
                    };

                    // Tier 1 -> Tier 2 BPF -> Tier 3 unfiltered user-space filtering
                    if !apply_filter(bpf_primary) {
                        let _ = apply_filter(bpf_fallback);
                    }

                    let handle = SendPcapHandle(raw_handle);
                    let lib_clone = Arc::clone(&lib);
                    let eng_clone = Arc::clone(&engine);
                    let st_clone = Arc::clone(&status);
                    if std::thread::Builder::new()
                        .name("photon-npcap-worker".into())
                        .spawn(move || {
                            npcap_worker_loop(handle, lib_clone, eng_clone, st_clone);
                        })
                        .is_ok()
                    {
                        opened_count += 1;
                    } else {
                        unsafe { (lib.pcap_close)(raw_handle) };
                    }
                }
            }
            curr = dev.next;
        }
    }

    unsafe { (lib.pcap_freealldevs)(alldevs) };

    if opened_count > 0 {
        if let Ok(mut st) = status.lock() {
            st.mode = "npcap".into();
            st.bind_address = format!("{opened_count} carte(s) Npcap");
            st.detail = format!(
                "Capture matérielle Npcap NDIS active sur {opened_count} interface(s) (UDP 5055-5060/4530-4540)"
            );
            st.needs_elevation = false;
        }
        true
    } else {
        false
    }
}

#[cfg(target_os = "windows")]
fn npcap_worker_loop(
    handle: npcap::SendPcapHandle,
    lib: Arc<npcap::NpcapLib>,
    engine: Arc<Mutex<AnalyzerEngine>>,
    status: Arc<Mutex<CaptureStatus>>,
) {
    use npcap::PcapPkthdr;

    let raw_handle = handle.0;
    let datalink = unsafe { (lib.pcap_datalink)(raw_handle) };
    let mut ipv4_fragments = Ipv4FragmentAssembler::new();
    let mut header_ptr: *mut PcapPkthdr = core::ptr::null_mut();
    let mut data_ptr: *const u8 = core::ptr::null();

    loop {
        let res = unsafe { (lib.pcap_next_ex)(raw_handle, &mut header_ptr, &mut data_ptr) };
        if res == 0 {
            continue;
        }
        if res < 0 {
            break;
        }
        if header_ptr.is_null() || data_ptr.is_null() {
            continue;
        }

        let caplen = unsafe { (*header_ptr).caplen as usize };
        if caplen == 0 || caplen > 65_535 {
            continue;
        }

        let frame = unsafe { core::slice::from_raw_parts(data_ptr, caplen) };
        let now_ms = now_unix_ms();
        if let Some(udp) =
            extract_photon_udp_from_datalink(Some(datalink), frame, &mut ipv4_fragments, now_ms)
        {
            if let Ok(mut st) = status.lock() {
                st.raw_datagrams_seen += 1;
            }
            if let Ok(mut eng) = engine.lock() {
                eng.ingest_udp_payload(&udp.payload, now_ms);
            }
        }
    }

    unsafe { (lib.pcap_close)(raw_handle) };
}

#[cfg(target_os = "windows")]
fn windows_raw_supervisor(engine: Arc<Mutex<AnalyzerEngine>>, status: Arc<Mutex<CaptureStatus>>) {
    use std::collections::HashSet;
    use std::time::Duration;

    let active_ips: Arc<Mutex<HashSet<Ipv4Addr>>> = Arc::new(Mutex::new(HashSet::new()));
    let active_v6_ips: Arc<Mutex<HashSet<Ipv6Addr>>> = Arc::new(Mutex::new(HashSet::new()));

    loop {
        let candidates = detect_candidate_ipv4s();
        let candidates_v6 = detect_candidate_ipv6s();
        if candidates.is_empty() && candidates_v6.is_empty() {
            let v4_empty = active_ips.lock().map(|g| g.is_empty()).unwrap_or(false);
            let v6_empty = active_v6_ips.lock().map(|g| g.is_empty()).unwrap_or(false);
            if v4_empty && v6_empty {
                if let Ok(mut st) = status.lock() {
                    if st.mode != "npcap" {
                        st.mode = "standby".into();
                        st.bind_address = "127.0.0.1:50560".into();
                        st.detail =
                            "Aucune interface réseau active détectée — mode veille prêt".into();
                        st.needs_elevation = false;
                    }
                }
            }
        }

        for ip in candidates {
            let should_spawn = active_ips
                .lock()
                .map(|mut active| active.insert(ip))
                .unwrap_or(false);
            if should_spawn {
                let eng_clone = Arc::clone(&engine);
                let st_clone = Arc::clone(&status);
                let active_clone = Arc::clone(&active_ips);
                if std::thread::Builder::new()
                    .name(format!("photon-raw-{ip}"))
                    .spawn(move || {
                        windows_raw_interface_loop(ip, eng_clone, st_clone);
                        if let Ok(mut active) = active_clone.lock() {
                            active.remove(&ip);
                        }
                    })
                    .is_err()
                {
                    if let Ok(mut active) = active_ips.lock() {
                        active.remove(&ip);
                    }
                }
            }
        }

        for ip6 in candidates_v6 {
            let should_spawn = active_v6_ips
                .lock()
                .map(|mut active| active.insert(ip6))
                .unwrap_or(false);
            if should_spawn {
                let eng_clone = Arc::clone(&engine);
                let st_clone = Arc::clone(&status);
                let active_clone = Arc::clone(&active_v6_ips);
                if std::thread::Builder::new()
                    .name(format!("photon-raw6-{ip6}"))
                    .spawn(move || {
                        windows_raw_v6_interface_loop(ip6, eng_clone, st_clone);
                        if let Ok(mut active) = active_clone.lock() {
                            active.remove(&ip6);
                        }
                    })
                    .is_err()
                {
                    if let Ok(mut active) = active_v6_ips.lock() {
                        active.remove(&ip6);
                    }
                }
            }
        }

        std::thread::sleep(Duration::from_secs(8));
    }
}

#[cfg(target_os = "windows")]
fn windows_raw_interface_loop(
    local_ip: Ipv4Addr,
    engine: Arc<Mutex<AnalyzerEngine>>,
    status: Arc<Mutex<CaptureStatus>>,
) {
    #[repr(C)]
    struct SockAddrIn {
        sin_family: u16,
        sin_port: u16,
        sin_addr: u32,
        sin_zero: [u8; 8],
    }

    #[link(name = "ws2_32")]
    extern "system" {
        fn socket(af: i32, sock_type: i32, protocol: i32) -> usize;
        fn bind(s: usize, name: *const SockAddrIn, namelen: i32) -> i32;
        fn setsockopt(s: usize, level: i32, optname: i32, optval: *const u8, optlen: i32) -> i32;
        fn WSAIoctl(
            s: usize,
            dwIoControlCode: u32,
            lpvInBuffer: *const u32,
            cbInBuffer: u32,
            lpvOutBuffer: *mut u32,
            cbOutBuffer: u32,
            lpcbBytesReturned: *mut u32,
            lpOverlapped: *mut core::ffi::c_void,
            lpCompletionRoutine: *mut core::ffi::c_void,
        ) -> i32;
        fn recv(s: usize, buf: *mut u8, len: i32, flags: i32) -> i32;
        fn closesocket(s: usize) -> i32;
        fn WSAGetLastError() -> i32;
    }

    const AF_INET: i32 = 2;
    const SOCK_RAW: i32 = 3;
    const IPPROTO_IP: i32 = 0;
    const IP_HDRINCL: i32 = 2;
    const SIO_RCVALL: u32 = 0x9800_0001;
    const SIO_UDP_CONNRESET: u32 = 0x9800_000C;
    const RCVALL_ON: u32 = 1;
    const INVALID_SOCKET: usize = usize::MAX;

    let _winsock_guard = UdpSocket::bind("127.0.0.1:0");

    // SAFETY: Standard Winsock2 calls with valid stack buffers and bounds-checked lengths.
    unsafe {
        let raw_sock = socket(AF_INET, SOCK_RAW, IPPROTO_IP);
        if raw_sock == INVALID_SOCKET {
            let code = WSAGetLastError();
            if let Ok(mut st) = status.lock() {
                if st.mode != "rawSocket" && st.mode != "npcap" {
                    st.mode = "standby".into();
                    st.bind_address = format!("{local_ip}:5056");
                    st.detail = format!(
                        "Mode standard non-administrateur (code Winsock {code}) — installez Npcap ou cliquez sur « Activer Capture Admin »"
                    );
                    st.needs_elevation = true;
                }
            }
            return;
        }

        let addr = SockAddrIn {
            sin_family: AF_INET as u16,
            sin_port: 0,
            sin_addr: u32::from_ne_bytes(local_ip.octets()),
            sin_zero: [0; 8],
        };

        if bind(
            raw_sock,
            &addr as *const SockAddrIn,
            core::mem::size_of::<SockAddrIn>() as i32,
        ) != 0
        {
            let code = WSAGetLastError();
            closesocket(raw_sock);
            if let Ok(mut st) = status.lock() {
                if st.mode != "rawSocket" && st.mode != "npcap" {
                    st.mode = "standby".into();
                    st.bind_address = format!("{local_ip}:5056");
                    st.detail = format!("Bind raw socket refusé (code {code}) — mode veille actif");
                    st.needs_elevation = true;
                }
            }
            return;
        }

        let hdrincl: u32 = 1;
        let _ = setsockopt(
            raw_sock,
            IPPROTO_IP,
            IP_HDRINCL,
            &hdrincl as *const u32 as *const u8,
            4,
        );

        let in_val: u32 = RCVALL_ON;
        let mut out_val: u32 = 0;
        let mut bytes_ret: u32 = 0;
        let ioctl_res = WSAIoctl(
            raw_sock,
            SIO_RCVALL,
            &in_val,
            4,
            &mut out_val,
            4,
            &mut bytes_ret,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
        );

        if ioctl_res != 0 {
            let code = WSAGetLastError();
            closesocket(raw_sock);
            if let Ok(mut st) = status.lock() {
                if st.mode != "rawSocket" && st.mode != "npcap" {
                    st.mode = "standby".into();
                    st.bind_address = format!("{local_ip}:5056");
                    st.detail = format!(
                        "SIO_RCVALL requiert les droits Administrateur (code {code}) — cliquez sur « Activer Capture Admin » ou installez Npcap"
                    );
                    st.needs_elevation = true;
                }
            }
            return;
        }

        // Suppress WSAECONNRESET (10054) when ICMP port unreachable is returned by remote endpoints
        let false_val: u32 = 0;
        let mut out_dummy: u32 = 0;
        let mut bytes_ret2: u32 = 0;
        let _ = WSAIoctl(
            raw_sock,
            SIO_UDP_CONNRESET,
            &false_val,
            4,
            &mut out_dummy,
            4,
            &mut bytes_ret2,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
        );

        if let Ok(mut st) = status.lock() {
            if st.mode != "npcap" {
                st.mode = "rawSocket".into();
                st.bind_address = format!("{local_ip}:5056");
                st.detail = format!(
                    "Capture directe Photon multi-interfaces active sur {local_ip} (UDP 5055-5060/4530-4540)"
                );
                st.needs_elevation = false;
            }
        }

        let mut buf = vec![0u8; 65_535];
        let mut ipv4_fragments = Ipv4FragmentAssembler::new();
        loop {
            let n = recv(raw_sock, buf.as_mut_ptr(), buf.len() as i32, 0);
            if n <= 0 {
                let err = WSAGetLastError();
                if is_transient_winsock_error(err) {
                    continue;
                }
                break;
            }
            let packet = &buf[..n as usize];
            let now_ms = now_unix_ms();
            if let Some(udp) = extract_udp_from_ip_packet(packet) {
                if is_photon_port(udp.src_port)
                    || is_photon_port(udp.dst_port)
                    || looks_like_photon(udp.payload)
                {
                    if let Ok(mut st) = status.lock() {
                        st.raw_datagrams_seen += 1;
                    }
                    if let Ok(mut eng) = engine.lock() {
                        eng.ingest_udp_payload(udp.payload, now_ms);
                    }
                }
            } else if let Some(udp) = ipv4_fragments.try_reassemble_ipv4_udp(packet, now_ms) {
                if matches_photon_dgram(&udp) {
                    if let Ok(mut st) = status.lock() {
                        st.raw_datagrams_seen += 1;
                    }
                    if let Ok(mut eng) = engine.lock() {
                        eng.ingest_udp_payload(&udp.payload, now_ms);
                    }
                }
            }
        }

        closesocket(raw_sock);
    }
}

#[cfg(target_os = "windows")]
fn windows_raw_v6_interface_loop(
    local_ip: Ipv6Addr,
    engine: Arc<Mutex<AnalyzerEngine>>,
    status: Arc<Mutex<CaptureStatus>>,
) {
    #[repr(C)]
    #[allow(dead_code)]
    struct SockAddrIn6 {
        sin6_family: u16,
        sin6_port: u16,
        sin6_flowinfo: u32,
        sin6_addr: [u8; 16],
        sin6_scope_id: u32,
    }

    #[link(name = "ws2_32")]
    extern "system" {
        fn socket(af: i32, sock_type: i32, protocol: i32) -> usize;
        fn bind(s: usize, name: *const SockAddrIn6, namelen: i32) -> i32;
        fn recv(s: usize, buf: *mut u8, len: i32, flags: i32) -> i32;
        fn closesocket(s: usize) -> i32;
        fn WSAGetLastError() -> i32;
    }

    const AF_INET6: i32 = 23;
    const SOCK_RAW: i32 = 3;
    const IPPROTO_UDP: i32 = 17;
    const INVALID_SOCKET: usize = usize::MAX;

    let _winsock_guard = UdpSocket::bind("[::]:0");

    // SAFETY: Standard Winsock2 IPv6 raw socket calls with valid 28-byte SockAddrIn6 and slice bounds.
    unsafe {
        // On Windows, AF_INET6 SOCK_RAW with IPPROTO_UDP delivers [UDP header (8B) | payload]
        // without needing SIO_RCVALL (which is only valid for IPv4 IPPROTO_IP/ICMP).
        let raw_sock = socket(AF_INET6, SOCK_RAW, IPPROTO_UDP);
        if raw_sock == INVALID_SOCKET {
            let code = WSAGetLastError();
            if let Ok(mut st) = status.lock() {
                if st.mode == "initializing" {
                    st.mode = "standby".into();
                    st.bind_address = format!("[{local_ip}]:5056");
                    st.detail = format!(
                        "Mode standard non-administrateur IPv6 (code Winsock {code}) — installez Npcap ou cliquez sur « Activer Capture Admin »"
                    );
                    st.needs_elevation = true;
                }
            }
            return;
        }

        let addr = SockAddrIn6 {
            sin6_family: AF_INET6 as u16,
            sin6_port: 0,
            sin6_flowinfo: 0,
            sin6_addr: local_ip.octets(),
            sin6_scope_id: 0,
        };

        if bind(
            raw_sock,
            &addr as *const SockAddrIn6,
            core::mem::size_of::<SockAddrIn6>() as i32,
        ) != 0
        {
            closesocket(raw_sock);
            return;
        }

        if let Ok(mut st) = status.lock() {
            if st.mode != "npcap" && st.mode != "rawSocket" {
                st.mode = "rawSocket".into();
                st.bind_address = format!("[{local_ip}]:5056");
                st.detail = format!(
                    "Capture directe Photon IPv6 active sur [{local_ip}] (UDP 5055-5060/4530-4540)"
                );
                st.needs_elevation = false;
            }
        }

        let mut buf = vec![0u8; 65_535];
        loop {
            let n = recv(raw_sock, buf.as_mut_ptr(), buf.len() as i32, 0);
            if n <= 0 {
                let err = WSAGetLastError();
                if is_transient_winsock_error(err) {
                    continue;
                }
                break;
            }
            let packet = &buf[..n as usize];
            let udp = extract_udp_from_ip_packet(packet)
                .filter(|u| {
                    is_photon_port(u.src_port)
                        || is_photon_port(u.dst_port)
                        || looks_like_photon(u.payload)
                })
                .or_else(|| extract_udp_segment_for_photon(packet));

            if let Some(udp) = udp {
                if let Ok(mut st) = status.lock() {
                    st.raw_datagrams_seen += 1;
                }
                if let Ok(mut eng) = engine.lock() {
                    eng.ingest_udp_payload(udp.payload, now_unix_ms());
                }
            }
        }

        closesocket(raw_sock);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use albion_core::photon::wrap_ipv4_udp_packet;

    #[test]
    fn parses_tcpdump_hex_and_locates_photon_udp_across_link_headers() {
        let _ = detect_candidate_ipv4s();
        let _ = detect_candidate_ipv6s();
        let payload = [241u8, 1, 2, 3, 4, 5];
        let ipv4 = wrap_ipv4_udp_packet(
            [193, 169, 238, 10],
            [192, 168, 1, 20],
            5056,
            55000,
            &payload,
        );
        // Prepend a 14-byte Ethernet header before the IPv4 packet
        let mut eth_frame = vec![0xaa; 14];
        eth_frame.extend_from_slice(&ipv4);

        let mut hex_line = String::from("  0x0000:  ");
        for pair in eth_frame.chunks(2) {
            if pair.len() == 2 {
                hex_line.push_str(&format!("{:02x}{:02x} ", pair[0], pair[1]));
            } else {
                hex_line.push_str(&format!("{:02x} ", pair[0]));
            }
        }

        let mut decoded = Vec::new();
        assert!(parse_tcpdump_hex_line(&hex_line, &mut decoded));
        assert_eq!(decoded, eth_frame);

        let udp = find_ipv4_udp_in_frame(&decoded).unwrap();
        assert_eq!(udp.src_port, 5056);
        assert_eq!(udp.payload, &payload);
    }

    #[test]
    fn validates_transient_winsock_error_classification() {
        assert!(is_transient_winsock_error(10054)); // WSAECONNRESET
        assert!(is_transient_winsock_error(10040)); // WSAEMSGSIZE
        assert!(is_transient_winsock_error(10035)); // WSAEWOULDBLOCK
        assert!(is_transient_winsock_error(10060)); // WSAETIMEDOUT
        assert!(is_transient_winsock_error(10004)); // WSAEINTR
        assert!(!is_transient_winsock_error(10009)); // WSAEBADF
        assert!(!is_transient_winsock_error(10013)); // WSAEACCES
        assert!(!is_transient_winsock_error(0));
    }

    #[test]
    fn extracts_photon_from_ethernet_with_single_and_qinq_vlan_tags() {
        let payload = [0x01u8, 0x02, 0x03, 0x04, 0x05, 0x06];
        let ipv4 = wrap_ipv4_udp_packet(
            [193, 169, 238, 10],
            [192, 168, 1, 20],
            5058,
            51000,
            &payload,
        );
        let mut frags = Ipv4FragmentAssembler::new();

        // Single 802.1Q VLAN tag (0x8100 -> 0x0800)
        let mut single_vlan = vec![0x11; 12];
        single_vlan.extend_from_slice(&0x8100u16.to_be_bytes());
        single_vlan.extend_from_slice(&[0x00, 0x64]); // PCP/VID
        single_vlan.extend_from_slice(&0x0800u16.to_be_bytes());
        single_vlan.extend_from_slice(&ipv4);
        let d1 =
            extract_photon_udp_from_datalink(Some(1), &single_vlan, &mut frags, 1_000).unwrap();
        assert_eq!(d1.src_port, 5058);
        assert_eq!(d1.payload, payload);

        // QinQ double VLAN tags (0x88A8 -> 0x8100 -> 0x0800)
        let mut qinq = vec![0x22; 12];
        qinq.extend_from_slice(&0x88A8u16.to_be_bytes());
        qinq.extend_from_slice(&[0x00, 0x10]);
        qinq.extend_from_slice(&0x8100u16.to_be_bytes());
        qinq.extend_from_slice(&[0x00, 0x20]);
        qinq.extend_from_slice(&0x0800u16.to_be_bytes());
        qinq.extend_from_slice(&ipv4);
        let d2 = extract_photon_udp_from_datalink(Some(1), &qinq, &mut frags, 1_100).unwrap();
        assert_eq!(d2.src_port, 5058);
        assert_eq!(d2.payload, payload);

        // Non-IP EtherType (ARP 0x0806) returns None even if payload looks similar
        let mut arp = vec![0x33; 12];
        arp.extend_from_slice(&0x0806u16.to_be_bytes());
        arp.extend_from_slice(&ipv4);
        assert!(extract_photon_udp_from_datalink(Some(1), &arp, &mut frags, 1_200).is_none());
    }

    #[test]
    fn extracts_photon_from_dlt_raw_ipv6_udp_with_trailing_padding() {
        let payload = [0x01u8, 0x05, 0x00, 0x01, 0xAA, 0xBB];
        let udp_len = (8 + payload.len()) as u16;
        let mut pkt = Vec::with_capacity(40 + udp_len as usize + 6);
        // IPv6 header (40 bytes)
        pkt.extend_from_slice(&[0x60, 0x00, 0x00, 0x00]);
        pkt.extend_from_slice(&udp_len.to_be_bytes());
        pkt.push(17); // NextHeader = UDP
        pkt.push(64); // Hop limit
        pkt.extend_from_slice(&[0x20, 0x01, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
        pkt.extend_from_slice(&[0x20, 0x01, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2]);
        // UDP header (8 bytes)
        pkt.extend_from_slice(&5056u16.to_be_bytes());
        pkt.extend_from_slice(&54321u16.to_be_bytes());
        pkt.extend_from_slice(&udp_len.to_be_bytes());
        pkt.extend_from_slice(&0u16.to_be_bytes());
        pkt.extend_from_slice(&payload);
        // Trailing link padding
        pkt.extend_from_slice(&[0u8; 6]);

        let mut frags = Ipv4FragmentAssembler::new();
        let dgram = extract_photon_udp_from_datalink(Some(12), &pkt, &mut frags, 2_000).unwrap();
        assert_eq!(dgram.src_port, 5056);
        assert_eq!(dgram.dst_port, 54321);
        assert_eq!(dgram.payload, payload);
    }

    #[test]
    fn reassembles_fragmented_ipv4_udp_over_ethernet_dlt() {
        let mut udp_segment = Vec::new();
        let payload: Vec<u8> = (0..40u8).collect();
        let udp_len = (8 + payload.len()) as u16;
        udp_segment.extend_from_slice(&5056u16.to_be_bytes());
        udp_segment.extend_from_slice(&52000u16.to_be_bytes());
        udp_segment.extend_from_slice(&udp_len.to_be_bytes());
        udp_segment.extend_from_slice(&0u16.to_be_bytes());
        udp_segment.extend_from_slice(&payload);

        let build_eth_ipv4_frag = |frag_off_bytes: usize, mf: bool, slice: &[u8]| -> Vec<u8> {
            let mut frame = vec![0x44; 12];
            frame.extend_from_slice(&0x0800u16.to_be_bytes());
            let total_len = (20 + slice.len()) as u16;
            let flags_frag =
                ((frag_off_bytes / 8) as u16 & 0x1FFF) | if mf { 0x2000 } else { 0x0000 };
            frame.extend_from_slice(&[0x45, 0x00]);
            frame.extend_from_slice(&total_len.to_be_bytes());
            frame.extend_from_slice(&0xABCDu16.to_be_bytes());
            frame.extend_from_slice(&flags_frag.to_be_bytes());
            frame.push(64);
            frame.push(17); // UDP
            frame.extend_from_slice(&0u16.to_be_bytes());
            frame.extend_from_slice(&[193, 169, 238, 10]);
            frame.extend_from_slice(&[192, 168, 1, 20]);
            frame.extend_from_slice(slice);
            frame.extend_from_slice(&[0u8; 10]); // Ethernet padding
            frame
        };

        let f1 = build_eth_ipv4_frag(0, true, &udp_segment[..24]);
        let f2 = build_eth_ipv4_frag(24, false, &udp_segment[24..]);

        let mut frags = Ipv4FragmentAssembler::new();
        assert!(extract_photon_udp_from_datalink(Some(1), &f1, &mut frags, 3_000).is_none());
        assert_eq!(frags.pending_count(), 1);

        let done = extract_photon_udp_from_datalink(Some(1), &f2, &mut frags, 3_050).unwrap();
        assert_eq!(frags.pending_count(), 0);
        assert_eq!(done.src_port, 5056);
        assert_eq!(done.dst_port, 52000);
        assert_eq!(done.payload, payload);
    }

    #[test]
    fn extracts_headerless_udp_segment_for_windows_ipv6_raw_socket() {
        let payload = [0x01u8, 0x02, 0xFE, 0xED];
        let udp_len = (8 + payload.len()) as u16;

        let mut photon_seg = Vec::new();
        photon_seg.extend_from_slice(&5056u16.to_be_bytes());
        photon_seg.extend_from_slice(&51000u16.to_be_bytes());
        photon_seg.extend_from_slice(&udp_len.to_be_bytes());
        photon_seg.extend_from_slice(&0u16.to_be_bytes());
        photon_seg.extend_from_slice(&payload);

        let parsed = extract_udp_segment_for_photon(&photon_seg).unwrap();
        assert_eq!(parsed.src_port, 5056);
        assert_eq!(parsed.dst_port, 51000);
        assert_eq!(parsed.payload, &payload);

        let mut non_photon_seg = photon_seg.clone();
        non_photon_seg[0..2].copy_from_slice(&1234u16.to_be_bytes());
        assert!(extract_udp_segment_for_photon(&non_photon_seg).is_none());
    }
}
