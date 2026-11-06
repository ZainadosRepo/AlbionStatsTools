//! Passive Photon UDP network capture on port `5056` (`SIO_RCVALL` raw socket on Windows +
//! local loopback relay listener on `127.0.0.1:50560`).

use albion_core::engine::AnalyzerEngine;
use albion_core::photon::{extract_udp_from_ip_packet, is_photon_port, looks_like_photon};
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureStatus {
    pub mode: String,
    pub bind_address: String,
    pub detail: String,
    pub raw_datagrams_seen: u64,
}

impl Default for CaptureStatus {
    fn default() -> Self {
        Self {
            mode: "initializing".into(),
            bind_address: "0.0.0.0:5056".into(),
            detail: "Initialisation de l'écoute passive Photon UDP (port 5056)…".into(),
            raw_datagrams_seen: 0,
        }
    }
}

pub fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Discovers the primary outbound IPv4 address used to reach Albion Online servers.
pub fn detect_outbound_ipv4() -> Option<Ipv4Addr> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("193.169.238.1:5056").ok()?;
    match sock.local_addr().ok()?.ip() {
        IpAddr::V4(ip) if !ip.is_loopback() && !ip.is_unspecified() => Some(ip),
        _ => None,
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

    // 2. Windows raw socket (`SIO_RCVALL`) passive listener on port 5056
    #[cfg(target_os = "windows")]
    {
        std::thread::Builder::new()
            .name("photon-raw-capture".into())
            .spawn(move || windows_raw_capture_loop(engine, status))
            .ok();
    }

    #[cfg(not(target_os = "windows"))]
    {
        if let Ok(mut st) = status.lock() {
            st.mode = "standby".into();
            st.bind_address = "127.0.0.1:50560".into();
            st.detail =
                "Mode veille actif (relais UDP local 127.0.0.1:50560 + simulateur Photon)".into();
        }
    }
}

#[cfg(target_os = "windows")]
fn windows_raw_capture_loop(engine: Arc<Mutex<AnalyzerEngine>>, status: Arc<Mutex<CaptureStatus>>) {
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

    let Some(local_ip) = detect_outbound_ipv4() else {
        if let Ok(mut st) = status.lock() {
            st.mode = "standby".into();
            st.bind_address = "127.0.0.1:50560".into();
            st.detail = "Aucune interface IPv4 active détectée — simulateur prêt".into();
        }
        return;
    };

    // Ensure Winsock is initialized by binding a transient std UdpSocket
    let _winsock_guard = UdpSocket::bind("127.0.0.1:0");

    // SAFETY: Standard Winsock2 calls with valid stack buffers and bounds-checked lengths.
    unsafe {
        let raw_sock = socket(AF_INET, SOCK_RAW, IPPROTO_IP);
        if raw_sock == INVALID_SOCKET {
            let code = WSAGetLastError();
            if let Ok(mut st) = status.lock() {
                st.mode = "standby".into();
                st.bind_address = format!("{local_ip}:5056");
                st.detail = format!(
                    "Mode standard non-administrateur (code Winsock {code}) — lancez en Administrateur pour la capture directe ou utilisez le simulateur"
                );
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
                st.mode = "standby".into();
                st.bind_address = format!("{local_ip}:5056");
                st.detail = format!("Bind raw socket refusé (code {code}) — mode veille actif");
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
                st.mode = "standby".into();
                st.bind_address = format!("{local_ip}:5056");
                st.detail = format!(
                    "SIO_RCVALL requiert les droits Administrateur (code {code}) — simulateur et relais prêts"
                );
            }
            return;
        }

        if let Ok(mut st) = status.lock() {
            st.mode = "rawSocket".into();
            st.bind_address = format!("{local_ip}:5056");
            st.detail =
                format!("Capture directe Photon active sur {local_ip} (UDP 5056/5055/4535)");
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
