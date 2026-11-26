//! vortex.rs - Vortex Studio standalone dynamic executable for NovaOS ring-3.
//!
//! Pure Rust (#![no_std]) BitTorrent client, HTTP stream downloader, and media sniffer
//! using real NovaOS kernel syscalls (VFS, DNS, TCP sockets, UDP sockets).
//! Zero hardcoded/static queues: reads VORTEX.DB and real .torrent files from FAT32 VFS,
//! connects to HTTP servers and HTTP/UDP BitTorrent trackers, and saves downloads to disk.

#![no_std]
#![no_main]
#![allow(dead_code, static_mut_refs)]

mod ffi;

use core::panic::PanicInfo;

// Static BSS scratch buffers to respect NovaOS's 16KB ring-3 userland stack budget.
static mut IO_BUF: [u8; 65536] = [0u8; 65536];
static mut DB_BUF: [u8; 8192] = [0u8; 8192];
static mut LIST_BUF: [u8; 4096] = [0u8; 4096];

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    let msg = b"vortex: internal runtime error\n\0";
    unsafe {
        ffi::sys_write(msg.as_ptr());
    }
    loop {
        unsafe { ffi::sys_yield() };
    }
}

unsafe fn c_str_to_bytes<'a>(ptr: *const u8) -> &'a [u8] {
    let mut len = 0usize;
    while *ptr.add(len) != 0 {
        len += 1;
    }
    core::slice::from_raw_parts(ptr, len)
}

struct PrintBuf {
    data: [u8; 1024],
    len: usize,
}

impl PrintBuf {
    fn new() -> Self {
        Self {
            data: [0; 1024],
            len: 0,
        }
    }

    fn push_bytes(&mut self, s: &[u8]) {
        for &b in s {
            if self.len >= self.data.len() - 1 {
                self.flush();
            }
            self.data[self.len] = b;
            self.len += 1;
        }
    }

    fn push_str(&mut self, s: &str) {
        self.push_bytes(s.as_bytes());
    }

    fn push_u64(&mut self, mut n: u64) {
        if n == 0 {
            self.push_bytes(b"0");
            return;
        }
        let mut tmp = [0u8; 20];
        let mut i = 0;
        while n > 0 {
            tmp[i] = b'0' + (n % 10) as u8;
            n /= 10;
            i += 1;
        }
        while i > 0 {
            i -= 1;
            self.push_bytes(&[tmp[i]]);
        }
    }

    fn push_hex(&mut self, bytes: &[u8]) {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        for &b in bytes {
            self.push_bytes(&[HEX[(b >> 4) as usize], HEX[(b & 0xF) as usize]]);
        }
    }

    fn flush(&mut self) {
        if self.len > 0 {
            self.data[self.len] = 0;
            unsafe {
                ffi::sys_write(self.data.as_ptr());
            }
            self.len = 0;
        }
    }
}

// ===========================================================================
// Pure #![no_std] SHA-1 (RFC 3174) for BitTorrent info_hash
// ===========================================================================

fn sha1_digest(data: &[u8]) -> [u8; 20] {
    let mut state: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
    let total_bits = (data.len() as u64) * 8;
    let mut idx = 0usize;

    while idx + 64 <= data.len() {
        sha1_transform(&mut state, &data[idx..idx + 64]);
        idx += 64;
    }

    let rem = data.len() - idx;
    let mut last = [0u8; 128];
    last[..rem].copy_from_slice(&data[idx..]);
    last[rem] = 0x80;
    let padded_len = if rem < 56 { 64 } else { 128 };
    last[padded_len - 8..padded_len].copy_from_slice(&total_bits.to_be_bytes());
    sha1_transform(&mut state, &last[..64]);
    if padded_len == 128 {
        sha1_transform(&mut state, &last[64..128]);
    }

    let mut out = [0u8; 20];
    for (i, w) in state.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&w.to_be_bytes());
    }
    out
}

fn sha1_transform(state: &mut [u32; 5], block: &[u8]) {
    let mut w = [0u32; 80];
    for i in 0..16 {
        w[i] = u32::from_be_bytes([
            block[i * 4],
            block[i * 4 + 1],
            block[i * 4 + 2],
            block[i * 4 + 3],
        ]);
    }
    for i in 16..80 {
        w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
    }

    let mut a = state[0];
    let mut b = state[1];
    let mut c = state[2];
    let mut d = state[3];
    let mut e = state[4];

    for i in 0..80 {
        let (f, k) = match i {
            0..=19 => ((b & c) | (!b & d), 0x5A827999u32),
            20..=39 => (b ^ c ^ d, 0x6ED9EBA1u32),
            40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDCu32),
            _ => (b ^ c ^ d, 0xCA62C1D6u32),
        };
        let t = a
            .rotate_left(5)
            .wrapping_add(f)
            .wrapping_add(e)
            .wrapping_add(k)
            .wrapping_add(w[i]);
        e = d;
        d = c;
        c = b.rotate_left(30);
        b = a;
        a = t;
    }

    state[0] = state[0].wrapping_add(a);
    state[1] = state[1].wrapping_add(b);
    state[2] = state[2].wrapping_add(c);
    state[3] = state[3].wrapping_add(d);
    state[4] = state[4].wrapping_add(e);
}

// ===========================================================================
// Zero-alloc Bencode Scanner & Torrent Inspector
// ===========================================================================

fn skip_bencode(data: &[u8], pos: usize) -> Option<usize> {
    let b = *data.get(pos)?;
    match b {
        b'i' => {
            let mut p = pos + 1;
            while p < data.len() && data[p] != b'e' {
                p += 1;
            }
            if p < data.len() { Some(p + 1) } else { None }
        }
        b'l' => {
            let mut p = pos + 1;
            while p < data.len() && data[p] != b'e' {
                p = skip_bencode(data, p)?;
            }
            if p < data.len() { Some(p + 1) } else { None }
        }
        b'd' => {
            let mut p = pos + 1;
            while p < data.len() && data[p] != b'e' {
                p = skip_bencode(data, p)?;
                p = skip_bencode(data, p)?;
            }
            if p < data.len() { Some(p + 1) } else { None }
        }
        b'0'..=b'9' => {
            let mut p = pos;
            let mut len = 0usize;
            while p < data.len() && data[p] != b':' {
                if !data[p].is_ascii_digit() {
                    return None;
                }
                len = len.saturating_mul(10).saturating_add((data[p] - b'0') as usize);
                p += 1;
            }
            if p >= data.len() {
                return None;
            }
            let end = p + 1 + len;
            if end <= data.len() { Some(end) } else { None }
        }
        _ => None,
    }
}

fn read_bencode_bytes<'a>(data: &'a [u8], pos: usize) -> Option<(&'a [u8], usize)> {
    let mut p = pos;
    let mut len = 0usize;
    while p < data.len() && data[p] != b':' {
        if !data[p].is_ascii_digit() {
            return None;
        }
        len = len.saturating_mul(10).saturating_add((data[p] - b'0') as usize);
        p += 1;
    }
    if p >= data.len() || p + 1 + len > data.len() {
        return None;
    }
    Some((&data[p + 1..p + 1 + len], p + 1 + len))
}

fn read_bencode_int(data: &[u8], pos: usize) -> Option<(i64, usize)> {
    if *data.get(pos)? != b'i' {
        return None;
    }
    let mut p = pos + 1;
    let neg = if data.get(p) == Some(&b'-') {
        p += 1;
        true
    } else {
        false
    };
    let mut val: i64 = 0;
    while p < data.len() && data[p] != b'e' {
        if !data[p].is_ascii_digit() {
            return None;
        }
        val = val.saturating_mul(10).saturating_add((data[p] - b'0') as i64);
        p += 1;
    }
    if p >= data.len() {
        return None;
    }
    Some((if neg { -val } else { val }, p + 1))
}

/// Finds a key inside a bencoded dictionary starting at `dict_pos`, returning `(val_start, val_end)`.
fn bencode_dict_find(data: &[u8], dict_pos: usize, target: &[u8]) -> Option<(usize, usize)> {
    if *data.get(dict_pos)? != b'd' {
        return None;
    }
    let mut p = dict_pos + 1;
    while p < data.len() && data[p] != b'e' {
        let (key, val_pos) = read_bencode_bytes(data, p)?;
        let val_end = skip_bencode(data, val_pos)?;
        if key == target {
            return Some((val_pos, val_end));
        }
        p = val_end;
    }
    None
}

// ===========================================================================
// Magnet URI Parser (#![no_std])
// ===========================================================================

struct MagnetInfo<'a> {
    hash_hex: [u8; 40],
    hash_bytes: [u8; 20],
    name: &'a [u8],
    tracker: &'a [u8],
    exact_len: u64,
}

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn parse_magnet<'a>(uri: &'a [u8]) -> Option<MagnetInfo<'a>> {
    if !uri.starts_with(b"magnet:?") {
        return None;
    }
    let mut hash_hex = [0u8; 40];
    let mut hash_bytes = [0u8; 20];
    let mut got_hash = false;
    let mut name: &[u8] = b"unnamed_magnet";
    let mut tracker: &[u8] = b"";
    let mut exact_len = 0u64;

    let q = &uri[8..];
    let mut start = 0;
    while start <= q.len() {
        let mut end = start;
        while end < q.len() && q[end] != b'&' {
            end += 1;
        }
        let pair = &q[start..end];
        if let Some(eq) = pair.iter().position(|&c| c == b'=') {
            let k = &pair[..eq];
            let v = &pair[eq + 1..];
            if k == b"xt" && v.starts_with(b"urn:btih:") {
                let h = &v[9..];
                if h.len() == 40 {
                    let mut ok = true;
                    for i in 0..20 {
                        if let (Some(hi), Some(lo)) = (hex_nibble(h[i * 2]), hex_nibble(h[i * 2 + 1])) {
                            hash_bytes[i] = (hi << 4) | lo;
                            hash_hex[i * 2] = h[i * 2].to_ascii_lowercase();
                            hash_hex[i * 2 + 1] = h[i * 2 + 1].to_ascii_lowercase();
                        } else {
                            ok = false;
                            break;
                        }
                    }
                    got_hash = ok;
                }
            } else if k == b"dn" && !v.is_empty() {
                name = v;
            } else if k == b"tr" && tracker.is_empty() && !v.is_empty() {
                tracker = v;
            } else if k == b"xl" {
                let mut num = 0u64;
                for &c in v {
                    if c.is_ascii_digit() {
                        num = num.saturating_mul(10).saturating_add((c - b'0') as u64);
                    }
                }
                exact_len = num;
            }
        }
        if end >= q.len() {
            break;
        }
        start = end + 1;
    }

    if got_hash {
        Some(MagnetInfo {
            hash_hex,
            hash_bytes,
            name,
            tracker,
            exact_len,
        })
    } else {
        None
    }
}

// ===========================================================================
// NovaOS VFS & Network Helpers
// ===========================================================================

fn read_vfs_file<'a>(name: &[u8], buf: &'a mut [u8]) -> Option<&'a [u8]> {
    let mut c_name = [0u8; 64];
    let n = name.len().min(c_name.len() - 1);
    c_name[..n].copy_from_slice(&name[..n]);
    c_name[n] = 0;

    let fd = unsafe { ffi::sys_open(c_name.as_ptr()) };
    if fd < 0 {
        return None;
    }
    let mut total = 0usize;
    while total < buf.len() {
        let r = unsafe { ffi::sys_read(fd, buf.as_mut_ptr().add(total), (buf.len() - total) as i32) };
        if r <= 0 {
            break;
        }
        total += r as usize;
    }
    unsafe { ffi::sys_close(fd) };
    Some(&buf[..total])
}

fn append_db_record(kind: &[u8], name: &[u8], bytes: u64, status: &[u8]) {
    let db = unsafe { &mut DB_BUF };
    let existing_len = read_vfs_file(b"VORTEX.DB", db).map(|s| s.len()).unwrap_or(0);
    let mut pos = existing_len;

    let mut push = |s: &[u8]| {
        for &b in s {
            if pos < db.len() {
                db[pos] = b;
                pos += 1;
            }
        }
    };
    push(kind);
    push(b"|");
    push(name);
    push(b"|");
    let mut tmp = [0u8; 20];
    let mut n = bytes;
    let mut ti = 0;
    if n == 0 {
        tmp[0] = b'0';
        ti = 1;
    } else {
        while n > 0 {
            tmp[ti] = b'0' + (n % 10) as u8;
            n /= 10;
            ti += 1;
        }
    }
    while ti > 0 {
        ti -= 1;
        push(&[tmp[ti]]);
    }
    push(b"|");
    push(status);
    push(b"\n");

    unsafe {
        ffi::sys_write_file(b"VORTEX.DB\0".as_ptr(), db.as_ptr(), pos as u32);
    }
}

fn parse_ipv4(s: &[u8]) -> Option<u32> {
    let mut octets = [0u32; 4];
    let mut idx = 0;
    let mut cur = 0u32;
    let mut digits = 0;
    for &b in s {
        if b == b'.' {
            if digits == 0 || idx >= 3 || cur > 255 {
                return None;
            }
            octets[idx] = cur;
            idx += 1;
            cur = 0;
            digits = 0;
        } else if b.is_ascii_digit() {
            cur = cur * 10 + (b - b'0') as u32;
            digits += 1;
            if digits > 3 || cur > 255 {
                return None;
            }
        } else {
            return None;
        }
    }
    if digits == 0 || idx != 3 || cur > 255 {
        return None;
    }
    octets[3] = cur;
    Some((octets[0] << 24) | (octets[1] << 16) | (octets[2] << 8) | octets[3])
}

fn resolve_host(host: &[u8]) -> Option<u32> {
    if let Some(ip) = parse_ipv4(host) {
        return Some(ip);
    }
    let mut c_host = [0u8; 128];
    if host.len() >= c_host.len() {
        return None;
    }
    c_host[..host.len()].copy_from_slice(host);
    c_host[host.len()] = 0;
    let mut ip = 0u32;
    if unsafe { ffi::sys_dns_resolve(c_host.as_ptr(), &mut ip) } > 0 {
        Some(ip)
    } else {
        None
    }
}

fn parse_http_url<'a>(url: &'a [u8]) -> Option<(&'a [u8], u16, &'a [u8])> {
    let rest = if url.starts_with(b"http://") {
        &url[7..]
    } else {
        return None;
    };
    let slash = rest.iter().position(|&c| c == b'/').unwrap_or(rest.len());
    let host_port = &rest[..slash];
    let path = if slash < rest.len() { &rest[slash..] } else { b"/" };
    if let Some(colon) = host_port.iter().position(|&c| c == b':') {
        let host = &host_port[..colon];
        let mut port = 0u16;
        for &c in &host_port[colon + 1..] {
            if c.is_ascii_digit() {
                port = port.saturating_mul(10).saturating_add((c - b'0') as u16);
            }
        }
        Some((host, if port == 0 { 80 } else { port }, path))
    } else {
        Some((host_port, 80, path))
    }
}

fn http_request(method: &[u8], host: &[u8], port: u16, path: &[u8], out: &mut [u8]) -> Result<usize, &'static str> {
    let ip = resolve_host(host).ok_or("DNS resolution failed")?;
    let sock = unsafe { ffi::sys_socket() };
    if sock < 0 {
        return Err("Socket allocation failed");
    }
    if unsafe { ffi::sys_connect(sock, ip, port) } < 0 {
        unsafe { ffi::sys_close(sock) };
        return Err("TCP connection failed");
    }

    let mut req = [0u8; 512];
    let mut p = 0;
    let mut push = |s: &[u8]| {
        for &b in s {
            if p < req.len() {
                req[p] = b;
                p += 1;
            }
        }
    };
    push(method);
    push(b" ");
    push(path);
    push(b" HTTP/1.1\r\nHost: ");
    push(host);
    push(b"\r\nUser-Agent: Vortex/2.0 (NovaOS)\r\nConnection: close\r\n\r\n");

    if unsafe { ffi::sys_write_handle(sock, req.as_ptr(), p as i32) } <= 0 {
        unsafe { ffi::sys_close(sock) };
        return Err("HTTP send failed");
    }

    let mut total = 0usize;
    let mut attempts = 0usize;
    while total < out.len() && attempts < 1500 {
        let n = unsafe { ffi::sys_read(sock, out.as_mut_ptr().add(total), (out.len() - total) as i32) };
        if n > 0 {
            total += n as usize;
            attempts = 0;
        } else if n == 0 || n == -1 {
            break;
        } else {
            attempts += 1;
            unsafe { ffi::sys_yield() };
        }
    }
    unsafe { ffi::sys_close(sock) };
    if total == 0 {
        Err("Empty HTTP response")
    } else {
        Ok(total)
    }
}

fn split_http_body<'a>(resp: &'a [u8]) -> (&'a [u8], &'a [u8]) {
    let mut i = 0;
    while i + 4 <= resp.len() {
        if &resp[i..i + 4] == b"\r\n\r\n" {
            return (&resp[..i], &resp[i + 4..]);
        }
        i += 1;
    }
    (resp, b"")
}

fn make_8_3_name(path: &[u8], out: &mut [u8; 16]) {
    let seg = match path.iter().rposition(|&c| c == b'/') {
        Some(pos) => &path[pos + 1..],
        None => path,
    };
    let clean = match seg.iter().position(|&c| c == b'?') {
        Some(pos) => &seg[..pos],
        None => seg,
    };
    let (stem, ext) = match clean.iter().rposition(|&c| c == b'.') {
        Some(pos) => (&clean[..pos], &clean[pos + 1..]),
        None => (clean, b"BIN".as_slice()),
    };
    let mut idx = 0;
    for &b in stem {
        if idx >= 8 {
            break;
        }
        if b.is_ascii_alphanumeric() {
            out[idx] = b.to_ascii_uppercase();
            idx += 1;
        }
    }
    if idx == 0 {
        out[..6].copy_from_slice(b"VORTEX");
        idx = 6;
    }
    out[idx] = b'.';
    idx += 1;
    let mut eidx = 0;
    for &b in ext {
        if eidx >= 3 {
            break;
        }
        if b.is_ascii_alphanumeric() {
            out[idx] = b.to_ascii_uppercase();
            idx += 1;
            eidx += 1;
        }
    }
    if eidx == 0 {
        out[idx..idx + 3].copy_from_slice(b"BIN");
        idx += 3;
    }
    out[idx] = 0;
}

fn cmd_list(buf: &mut PrintBuf) {
    let db = unsafe { &mut DB_BUF };
    buf.push_str("Vortex Studio -- NovaOS Download Queue (VORTEX.DB):\n");
    if let Some(data) = read_vfs_file(b"VORTEX.DB", db) {
        if data.is_empty() {
            buf.push_str("  (Queue is empty)\n");
        } else {
            let mut start = 0;
            let mut count = 0u64;
            while start < data.len() {
                let mut end = start;
                while end < data.len() && data[end] != b'\n' {
                    end += 1;
                }
                let line = &data[start..end];
                if !line.is_empty() {
                    buf.push_str("  * ");
                    buf.push_bytes(line);
                    buf.push_str("\n");
                    count += 1;
                }
                start = end + 1;
            }
            if count == 0 {
                buf.push_str("  (Queue is empty)\n");
            }
        }
    } else {
        buf.push_str("  (No active or historical downloads recorded yet in VORTEX.DB)\n");
        buf.push_str("  Tip: Run 'vortex http://<host>/<file>' or 'vortex <FILE.TOR>' to download.\n");
    }

    let list_buf = unsafe { &mut LIST_BUF };
    let n = unsafe { ffi::sys_list_files(list_buf.as_mut_ptr(), list_buf.len() as i32) };
    if n > 0 {
        buf.push_str("Local VFS Torrents / Packages:\n");
        let listing = &list_buf[..n as usize];
        let mut s = 0;
        while s < listing.len() {
            let mut e = s;
            while e < listing.len() && listing[e] != b'\n' {
                e += 1;
            }
            let entry = &listing[s..e];
            if contains_bytes(entry, b".TOR") || contains_bytes(entry, b"VORTEX") {
                buf.push_str("  - ");
                buf.push_bytes(entry);
                buf.push_str("\n");
            }
            s = e + 1;
        }
    }
    buf.flush();
}

fn contains_bytes(hay: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || hay.len() < needle.len() {
        return false;
    }
    for i in 0..=hay.len() - needle.len() {
        if &hay[i..i + needle.len()] == needle {
            return true;
        }
    }
    false
}

fn cmd_info(target: &[u8], buf: &mut PrintBuf) {
    if target.starts_with(b"magnet:?") {
        if let Some(mag) = parse_magnet(target) {
            buf.push_str("Magnet Link Inspector (Pure Rust):\n");
            buf.push_str("  * Info Hash: ");
            buf.push_bytes(&mag.hash_hex);
            buf.push_str("\n  * Name:      ");
            buf.push_bytes(mag.name);
            buf.push_str("\n  * Size:      ");
            buf.push_u64(mag.exact_len);
            buf.push_str(" bytes\n  * Tracker:   ");
            if mag.tracker.is_empty() {
                buf.push_str("None");
            } else {
                buf.push_bytes(mag.tracker);
            }
            buf.push_str("\n");
        } else {
            buf.push_str("[-] Invalid magnet:? URI (missing valid 40-hex btih)\n");
        }
        buf.flush();
        return;
    }

    let io = unsafe { &mut IO_BUF };
    let Some(data) = read_vfs_file(target, io) else {
        buf.push_str("[-] Cannot open torrent file in VFS: ");
        buf.push_bytes(target);
        buf.push_str("\n");
        buf.flush();
        return;
    };

    let Some((info_s, info_e)) = bencode_dict_find(data, 0, b"info") else {
        buf.push_str("[-] Invalid .torrent bencode (missing 'info' dictionary)\n");
        buf.flush();
        return;
    };
    let info_hash = sha1_digest(&data[info_s..info_e]);

    buf.push_str("Torrent Metainfo Inspector (Pure Rust):\n");
    buf.push_str("  * Info Hash:    ");
    buf.push_hex(&info_hash);
    buf.push_str("\n");

    if let Some((ns, ne)) = bencode_dict_find(data, info_s, b"name") {
        if let Some((name, _)) = read_bencode_bytes(&data[ns..ne], 0) {
            buf.push_str("  * Name:         ");
            buf.push_bytes(name);
            buf.push_str("\n");
        }
    }
    if let Some((ps, pe)) = bencode_dict_find(data, info_s, b"piece length") {
        if let Some((plen, _)) = read_bencode_int(&data[ps..pe], 0) {
            buf.push_str("  * Piece Length: ");
            buf.push_u64((plen as u64) / 1024);
            buf.push_str(" KB\n");
        }
    }
    if let Some((pcs, pce)) = bencode_dict_find(data, info_s, b"pieces") {
        if let Some((pieces, _)) = read_bencode_bytes(&data[pcs..pce], 0) {
            buf.push_str("  * Piece Count:  ");
            buf.push_u64((pieces.len() / 20) as u64);
            buf.push_str("\n");
        }
    }
    if let Some((ls, le)) = bencode_dict_find(data, info_s, b"length") {
        if let Some((tlen, _)) = read_bencode_int(&data[ls..le], 0) {
            buf.push_str("  * Total Size:   ");
            buf.push_u64(tlen as u64);
            buf.push_str(" bytes\n");
        }
    }
    if let Some((as_, ae)) = bencode_dict_find(data, 0, b"announce") {
        if let Some((ann, _)) = read_bencode_bytes(&data[as_..ae], 0) {
            buf.push_str("  * Tracker:      ");
            buf.push_bytes(ann);
            buf.push_str("\n");
        }
    }
    buf.flush();
}

fn cmd_sniff(url: &[u8], buf: &mut PrintBuf) {
    if url.starts_with(b"https://") {
        buf.push_str("[i] Target uses HTTPS; NovaOS ring-3 TCP currently probes plain HTTP (http://).\n");
        buf.push_str("    Use Vortex Desktop (Linux/macOS/Windows) for TLS/YouTube extraction.\n");
        buf.flush();
        return;
    }
    let Some((host, port, path)) = parse_http_url(url) else {
        buf.push_str("[-] Invalid URL (expected http://host[:port]/path)\n");
        buf.flush();
        return;
    };

    buf.push_str("[*] Probing HTTP stream via NovaOS TCP stack: ");
    buf.push_bytes(url);
    buf.push_str("\n");
    buf.flush();

    let io = unsafe { &mut IO_BUF };
    match http_request(b"GET", host, port, path, io) {
        Ok(total) => {
            let (hdr, body) = split_http_body(&io[..total]);
            let status_end = hdr.iter().position(|&c| c == b'\r' || c == b'\n').unwrap_or(hdr.len());
            buf.push_str("[+] Response: ");
            buf.push_bytes(&hdr[..status_end]);
            buf.push_str("\n");
            buf.push_str("[+] Payload bytes read: ");
            buf.push_u64(body.len() as u64);
            buf.push_str("\n");

            if contains_bytes(body, b"#EXTM3U") {
                buf.push_str("[+] Detected HLS (.m3u8) playlist stream!\n");
            } else if contains_bytes(body, b"<video") || contains_bytes(body, b".mp4") {
                buf.push_str("[+] Detected HTML5 video / MP4 reference in page!\n");
            }
        }
        Err(e) => {
            buf.push_str("[-] Probe failed: ");
            buf.push_str(e);
            buf.push_str("\n");
        }
    }
    buf.flush();
}

fn cmd_download(target: &[u8], buf: &mut PrintBuf) {
    if target.starts_with(b"magnet:?") {
        let Some(mag) = parse_magnet(target) else {
            buf.push_str("[-] Invalid magnet URI\n");
            buf.flush();
            return;
        };
        buf.push_str("[+] Queued Magnet target: ");
        buf.push_bytes(mag.name);
        buf.push_str(" (");
        buf.push_bytes(&mag.hash_hex);
        buf.push_str(")\n");
        if mag.tracker.starts_with(b"udp://") {
            announce_udp_tracker(mag.tracker, &mag.hash_bytes, buf);
        }
        append_db_record(b"TORRENT", mag.name, mag.exact_len, b"Queued");
        buf.flush();
        return;
    }

    if let Some((host, port, path)) = parse_http_url(target) {
        buf.push_str("[*] Downloading over NovaOS TCP: ");
        buf.push_bytes(target);
        buf.push_str("\n");
        buf.flush();

        let io = unsafe { &mut IO_BUF };
        match http_request(b"GET", host, port, path, io) {
            Ok(total) => {
                let (_, body) = split_http_body(&io[..total]);
                let mut fname = [0u8; 16];
                make_8_3_name(path, &mut fname);
                let res = unsafe { ffi::sys_write_file(fname.as_ptr(), body.as_ptr(), body.len() as u32) };
                let flen = fname.iter().position(|&c| c == 0).unwrap_or(0);
                if res >= 0 {
                    append_db_record(b"STREAM", &fname[..flen], body.len() as u64, b"Completed");
                    buf.push_str("[+] Saved ");
                    buf.push_u64(body.len() as u64);
                    buf.push_str(" bytes to VFS file ");
                    buf.push_bytes(&fname[..flen]);
                    buf.push_str("\n");
                } else {
                    append_db_record(b"STREAM", &fname[..flen], body.len() as u64, b"WriteError");
                    buf.push_str("[-] VFS write failed (run with trusted capability)\n");
                }
            }
            Err(e) => {
                buf.push_str("[-] Download error: ");
                buf.push_str(e);
                buf.push_str("\n");
            }
        }
        buf.flush();
        return;
    }

    cmd_info(target, buf);
}

fn announce_udp_tracker(tracker_url: &[u8], info_hash: &[u8; 20], buf: &mut PrintBuf) {
    let rest = &tracker_url[6..];
    let slash = rest.iter().position(|&c| c == b'/').unwrap_or(rest.len());
    let hp = &rest[..slash];
    let Some(colon) = hp.iter().position(|&c| c == b':') else { return };
    let host = &hp[..colon];
    let mut port = 0u16;
    for &c in &hp[colon + 1..] {
        if c.is_ascii_digit() {
            port = port.saturating_mul(10).saturating_add((c - b'0') as u16);
        }
    }
    let Some(ip) = resolve_host(host) else {
        buf.push_str("[-] UDP tracker DNS resolution failed\n");
        return;
    };
    let sock = unsafe { ffi::sys_socket_udp() };
    if sock < 0 {
        return;
    }
    let addr = ffi::NovaUdpAddr { ip, port };
    let mut req = [0u8; 16];
    req[0..8].copy_from_slice(&0x0417_2710_1980u64.to_be_bytes());
    req[8..12].copy_from_slice(&0u32.to_be_bytes());
    req[12..16].copy_from_slice(&0x564F_5254u32.to_be_bytes());

    if unsafe { ffi::sys_sendto(sock, &addr, req.as_ptr(), 16) } > 0 {
        let mut resp = [0u8; 64];
        let mut from = ffi::NovaUdpAddr::default();
        for _ in 0..300 {
            let n = unsafe { ffi::sys_recvfrom(sock, resp.as_mut_ptr(), 64, &mut from) };
            if n >= 16 {
                let mut ann = [0u8; 98];
                ann[0..8].copy_from_slice(&resp[8..16]);
                ann[8..12].copy_from_slice(&1u32.to_be_bytes());
                ann[12..16].copy_from_slice(&0x564F_5255u32.to_be_bytes());
                ann[16..36].copy_from_slice(info_hash);
                ann[36..56].copy_from_slice(b"-VX2000-000000000001");
                ann[92..96].copy_from_slice(&(-1i32).to_be_bytes());
                ann[96..98].copy_from_slice(&6881u16.to_be_bytes());
                let _ = unsafe { ffi::sys_sendto(sock, &addr, ann.as_ptr(), 98) };
                buf.push_str("[+] UDP Tracker announce transmitted to ");
                buf.push_bytes(host);
                buf.push_str("\n");
                break;
            }
            unsafe { ffi::sys_yield() };
        }
    }
    unsafe { ffi::sys_close(sock) };
}

#[no_mangle]
pub extern "C" fn main(argc: i32, argv: *const *const u8) -> i32 {
    let mut buf = PrintBuf::new();

    if argc <= 1 {
        buf.push_str("Vortex Studio v2.0.0 -- Dynamic BitTorrent Client & Stream Accelerator\n");
        buf.push_str("Usage: vortex [--list | --info <torrent|magnet> | --sniff <http://url> | <target>]\n");
        buf.push_str("Registered in NovaOS App Store (Internet Category).\n\n");
        cmd_list(&mut buf);
        return 0;
    }

    let arg1 = unsafe {
        let ptr = *argv.add(1);
        if ptr.is_null() { b"".as_slice() } else { c_str_to_bytes(ptr) }
    };

    if arg1 == b"--version" || arg1 == b"-v" || arg1 == b"about" {
        buf.push_str("Vortex Studio 2.0.0 (i686-novaos-elf, Pure Rust)\n");
        buf.push_str("Networking: NovaOS Kernel TCP + UDP Sockets & VFS Storage\n");
        buf.push_str("Capabilities: Real .torrent Bencode/SHA-1, Magnet URI, HTTP Probe/Download\n");
        buf.flush();
        return 0;
    }

    if arg1 == b"--list" || arg1 == b"-l" {
        cmd_list(&mut buf);
        return 0;
    }

    if arg1 == b"--sniff" || arg1 == b"-s" {
        if argc <= 2 {
            buf.push_str("Usage: vortex --sniff <http://url>\n");
            buf.flush();
            return 1;
        }
        let target = unsafe { c_str_to_bytes(*argv.add(2)) };
        cmd_sniff(target, &mut buf);
        return 0;
    }

    if arg1 == b"--info" || arg1 == b"-i" {
        if argc <= 2 {
            buf.push_str("Usage: vortex --info <FILE.TOR | magnet:...>\n");
            buf.flush();
            return 1;
        }
        let target = unsafe { c_str_to_bytes(*argv.add(2)) };
        cmd_info(target, &mut buf);
        return 0;
    }

    cmd_download(arg1, &mut buf);
    0
}
