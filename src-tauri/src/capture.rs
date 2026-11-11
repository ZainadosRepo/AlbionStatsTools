//! Passive Photon UDP network capture on port `5056`:
//! - Windows: multi-interface `SIO_RCVALL` raw socket supervisor (auto-attaches VPN & LAN adapters)
//! - macOS: passive BPF capture via `/usr/sbin/tcpdump` when `/dev/bpf*` access is available
//! - All platforms: local loopback relay listener on `127.0.0.1:50560` + built-in wire simulator.

use albion_core::engine::AnalyzerEngine;
#[cfg(any(target_os = "macos", test))]
use albion_core::photon::UdpDatagramSlice;
use albion_core::photon::{extract_udp_from_ip_packet, is_photon_port, looks_like_photon};
use serde::{Deserialize, Serialize};
#[cfg(any(target_os = "windows", test))]
use std::net::{IpAddr, Ipv4Addr};
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
            detail: "Initialisation de l'écoute passive Photon UDP (port 5056)…".into(),
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

/// Scans a link-layer frame (Ethernet / BSD loopback / PKTAP) for an IPv4/IPv6 Photon UDP packet.
#[cfg(any(target_os = "macos", test))]
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
            st.detail =
                "Mode veille actif (relais UDP local 127.0.0.1:50560 + simulateur Photon)".into();
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
            "Mode veille macOS (relais UDP 127.0.0.1:50560 + vérification BPF port 5056…)".into();
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
            "udp and (port 5056 or port 5055 or port 4535)",
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
    let mut promoted_live = false;

    for line in reader.lines().map_while(Result::ok) {
        if !promoted_live {
            promoted_live = true;
            if let Ok(mut st) = status.lock() {
                st.mode = "rawSocket".into();
                st.bind_address = "any:5056".into();
                st.detail = "Capture passive macOS BPF active (UDP 5056/5055/4535)".into();
            }
        }
        if parse_tcpdump_hex_line(&line, &mut frame) {
            continue;
        }
        if !frame.is_empty() {
            if let Some(udp) = find_ipv4_udp_in_frame(&frame) {
                if let Ok(mut st) = status.lock() {
                    st.raw_datagrams_seen += 1;
                }
                if let Ok(mut eng) = engine.lock() {
                    eng.ingest_udp_payload(udp.payload, now_unix_ms());
                }
            }
            frame.clear();
        }
    }

    let _ = child.wait();
    if let Ok(mut st) = status.lock() {
        st.mode = "standby".into();
        st.bind_address = "127.0.0.1:50560".into();
        st.detail = "Mode veille macOS (relais UDP 127.0.0.1:50560 + simulateur — installez ChmodBPF pour la capture directe)".into();
    }
}

#[cfg(target_os = "windows")]
fn windows_capture_supervisor(
    engine: Arc<Mutex<AnalyzerEngine>>,
    status: Arc<Mutex<CaptureStatus>>,
) {
    use std::collections::HashSet;
    use std::time::Duration;

    let active_ips: Arc<Mutex<HashSet<Ipv4Addr>>> = Arc::new(Mutex::new(HashSet::new()));

    loop {
        let candidates = detect_candidate_ipv4s();
        if candidates.is_empty() {
            if let Ok(active) = active_ips.lock() {
                if active.is_empty() {
                    if let Ok(mut st) = status.lock() {
                        st.mode = "standby".into();
                        st.bind_address = "127.0.0.1:50560".into();
                        st.detail =
                            "Aucune interface IPv4 active détectée — simulateur prêt".into();
                        st.needs_elevation = false;
                    }
                }
            }
        }

        for ip in candidates {
            let should_spawn = if let Ok(mut active) = active_ips.lock() {
                active.insert(ip)
            } else {
                false
            };
            if should_spawn {
                let eng_clone = Arc::clone(&engine);
                let st_clone = Arc::clone(&status);
                let active_clone = Arc::clone(&active_ips);
                std::thread::Builder::new()
                    .name(format!("photon-raw-{ip}"))
                    .spawn(move || {
                        windows_raw_interface_loop(ip, eng_clone, st_clone);
                        if let Ok(mut active) = active_clone.lock() {
                            active.remove(&ip);
                        }
                    })
                    .ok();
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
    const RCVALL_ON: u32 = 1;
    const INVALID_SOCKET: usize = usize::MAX;

    let _winsock_guard = UdpSocket::bind("127.0.0.1:0");

    // SAFETY: Standard Winsock2 calls with valid stack buffers and bounds-checked lengths.
    unsafe {
        let raw_sock = socket(AF_INET, SOCK_RAW, IPPROTO_IP);
        if raw_sock == INVALID_SOCKET {
            let code = WSAGetLastError();
            if let Ok(mut st) = status.lock() {
                if st.mode != "rawSocket" {
                    st.mode = "standby".into();
                    st.bind_address = format!("{local_ip}:5056");
                    st.detail = format!(
                        "Mode standard non-administrateur (code Winsock {code}) — cliquez sur « Activer Capture Admin »"
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
                if st.mode != "rawSocket" {
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
                if st.mode != "rawSocket" {
                    st.mode = "standby".into();
                    st.bind_address = format!("{local_ip}:5056");
                    st.detail = format!(
                        "SIO_RCVALL requiert les droits Administrateur (code {code}) — cliquez sur « Activer Capture Admin »"
                    );
                    st.needs_elevation = true;
                }
            }
            return;
        }

        if let Ok(mut st) = status.lock() {
            st.mode = "rawSocket".into();
            st.bind_address = format!("{local_ip}:5056");
            st.detail = format!(
                "Capture directe Photon multi-interfaces active sur {local_ip} (UDP 5056/5055/4535)"
            );
            st.needs_elevation = false;
        }

        let mut buf = vec![0u8; 65_535];
        loop {
            let n = recv(raw_sock, buf.as_mut_ptr(), buf.len() as i32, 0);
            if n <= 0 {
                break;
            }
            let packet = &buf[..n as usize];
            let Some(udp) = extract_udp_from_ip_packet(packet) else {
                continue;
            };
            if is_photon_port(udp.src_port)
                || is_photon_port(udp.dst_port)
                || looks_like_photon(udp.payload)
            {
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
}
