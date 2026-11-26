//! vortex-ui/src/image_decode.rs
//! Pure-Rust PNG (via miniz_oxide) and Baseline JPEG decoder for rendering real video thumbnails.

#[derive(Clone, Debug)]
pub struct DecodedImage {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u32>, // 0xAARRGGBB
}

/// Decodes PNG or baseline JPEG bytes into an ARGB pixel buffer.
pub fn decode_image(bytes: &[u8]) -> Option<DecodedImage> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        decode_png(bytes)
    } else if bytes.starts_with(&[0xFF, 0xD8]) {
        decode_jpeg(bytes)
    } else {
        None
    }
}

fn decode_png(data: &[u8]) -> Option<DecodedImage> {
    let mut pos = 8usize;
    let mut width = 0usize;
    let mut height = 0usize;
    let mut bit_depth = 0u8;
    let mut color_type = 0u8;
    let mut idat = Vec::new();

    while pos + 12 <= data.len() {
        let len = u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]) as usize;
        let ctype = &data[pos + 4..pos + 8];
        let payload_start = pos + 8;
        let payload_end = payload_start.checked_add(len)?;
        if payload_end + 4 > data.len() {
            break;
        }
        let payload = &data[payload_start..payload_end];
        match ctype {
            b"IHDR" if payload.len() >= 13 => {
                width = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]) as usize;
                height = u32::from_be_bytes([payload[4], payload[5], payload[6], payload[7]]) as usize;
                bit_depth = payload[8];
                color_type = payload[9];
            }
            b"IDAT" => idat.extend_from_slice(payload),
            b"IEND" => break,
            _ => {}
        }
        pos = payload_end + 4;
    }

    if width == 0 || height == 0 || width > 4096 || height > 4096 || bit_depth != 8 {
        return None;
    }
    let bpp = match color_type {
        0 => 1, // Grayscale
        2 => 3, // RGB
        4 => 2, // Gray+Alpha
        6 => 4, // RGBA
        _ => return None,
    };

    let raw = miniz_oxide::inflate::decompress_to_vec_zlib(&idat).ok()?;
    let stride = width * bpp;
    if raw.len() < height * (stride + 1) {
        return None;
    }

    let mut prev_row = vec![0u8; stride];
    let mut cur_row = vec![0u8; stride];
    let mut pixels = vec![0xFF000000u32; width * height];

    for y in 0..height {
        let row_start = y * (stride + 1);
        let filter = raw[row_start];
        let src = &raw[row_start + 1..row_start + 1 + stride];

        for i in 0..stride {
            let a = if i >= bpp { cur_row[i - bpp] } else { 0 };
            let b = prev_row[i];
            let c = if i >= bpp { prev_row[i - bpp] } else { 0 };
            let recon = match filter {
                0 => src[i],
                1 => src[i].wrapping_add(a),
                2 => src[i].wrapping_add(b),
                3 => src[i].wrapping_add((((a as u16) + (b as u16)) / 2) as u8),
                4 => src[i].wrapping_add(paeth(a, b, c)),
                _ => return None,
            };
            cur_row[i] = recon;
        }

        for x in 0..width {
            let off = x * bpp;
            let (r, g, b, a) = match color_type {
                0 => (cur_row[off], cur_row[off], cur_row[off], 255),
                2 => (cur_row[off], cur_row[off + 1], cur_row[off + 2], 255),
                4 => (cur_row[off], cur_row[off], cur_row[off], cur_row[off + 1]),
                6 => (cur_row[off], cur_row[off + 1], cur_row[off + 2], cur_row[off + 3]),
                _ => (0, 0, 0, 255),
            };
            pixels[y * width + x] =
                ((a as u32) << 24) | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32);
        }
        prev_row.copy_from_slice(&cur_row);
    }

    Some(DecodedImage {
        width,
        height,
        pixels,
    })
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = (a as i32) + (b as i32) - (c as i32);
    let pa = (p - (a as i32)).abs();
    let pb = (p - (b as i32)).abs();
    let pc = (p - (c as i32)).abs();
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

// ---------- Pure-Rust Baseline JPEG Decoder (SOF0) ----------

const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

#[derive(Clone, Default)]
struct HuffTable {
    codes: Vec<(u8, u16, u8)>, // (bit_len, code, symbol)
}

impl HuffTable {
    fn build(counts: &[u8; 16], symbols: &[u8]) -> Self {
        let mut codes = Vec::with_capacity(symbols.len());
        let mut code = 0u16;
        let mut sym_idx = 0usize;
        for (i, &cnt) in counts.iter().enumerate() {
            let bit_len = (i + 1) as u8;
            for _ in 0..cnt {
                if sym_idx < symbols.len() {
                    codes.push((bit_len, code, symbols[sym_idx]));
                    sym_idx += 1;
                }
                code = code.wrapping_add(1);
            }
            code <<= 1;
        }
        Self { codes }
    }
}

#[derive(Clone, Copy, Default)]
struct ComponentInfo {
    id: u8,
    h_samp: usize,
    v_samp: usize,
    qt_id: usize,
    dc_id: usize,
    ac_id: usize,
    dc_pred: i32,
}

struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    bit_buf: u32,
    bit_count: u8,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            bit_buf: 0,
            bit_count: 0,
        }
    }

    fn next_bit(&mut self) -> Option<u32> {
        if self.bit_count == 0 {
            while self.bit_count < 24 && self.pos < self.data.len() {
                let b = self.data[self.pos];
                self.pos += 1;
                if b == 0xFF {
                    if self.pos < self.data.len() {
                        let next = self.data[self.pos];
                        if next == 0x00 {
                            self.pos += 1;
                        } else if (0xD0..=0xD7).contains(&next) {
                            // RSTn marker: consume and reset alignment
                            self.pos += 1;
                            continue;
                        } else {
                            self.pos -= 1;
                            break;
                        }
                    }
                }
                self.bit_buf = (self.bit_buf << 8) | (b as u32);
                self.bit_count += 8;
            }
            if self.bit_count == 0 {
                return None;
            }
        }
        self.bit_count -= 1;
        Some((self.bit_buf >> self.bit_count) & 1)
    }

    fn read_bits(&mut self, n: u8) -> Option<u16> {
        let mut v = 0u16;
        for _ in 0..n {
            v = (v << 1) | (self.next_bit()? as u16);
        }
        Some(v)
    }

    fn decode_huff(&mut self, table: &HuffTable) -> Option<u8> {
        let mut code = 0u16;
        let mut idx = 0usize;
        for len in 1..=16u8 {
            code = (code << 1) | (self.next_bit()? as u16);
            while idx < table.codes.len() && table.codes[idx].0 < len {
                idx += 1;
            }
            let mut j = idx;
            while j < table.codes.len() && table.codes[j].0 == len {
                if table.codes[j].1 == code {
                    return Some(table.codes[j].2);
                }
                j += 1;
            }
        }
        None
    }

    fn align_byte(&mut self) {
        let drop = self.bit_count % 8;
        self.bit_count -= drop;
        self.bit_buf &= (1u32 << self.bit_count).wrapping_sub(1);
    }
}

fn receive_extend(reader: &mut BitReader<'_>, n: u8) -> Option<i32> {
    if n == 0 {
        return Some(0);
    }
    let val = reader.read_bits(n)? as i32;
    let threshold = 1 << (n - 1);
    if val < threshold {
        Some(val - ((1 << n) - 1))
    } else {
        Some(val)
    }
}

fn decode_jpeg(data: &[u8]) -> Option<DecodedImage> {
    let mut pos = 2usize; // skip SOI (0xFF 0xD8)
    let mut qtables = [[0i32; 64]; 4];
    let mut dc_tables = [HuffTable::default(), HuffTable::default(), HuffTable::default(), HuffTable::default()];
    let mut ac_tables = [HuffTable::default(), HuffTable::default(), HuffTable::default(), HuffTable::default()];
    let mut width = 0usize;
    let mut height = 0usize;
    let mut comps: Vec<ComponentInfo> = Vec::new();
    let mut restart_interval = 0usize;
    let mut scan_start = 0usize;

    while pos + 4 <= data.len() {
        if data[pos] != 0xFF {
            pos += 1;
            continue;
        }
        while pos < data.len() && data[pos] == 0xFF {
            pos += 1;
        }
        if pos >= data.len() {
            break;
        }
        let marker = data[pos];
        pos += 1;
        if marker == 0xD9 || marker == 0x00 {
            break;
        }
        if (0xD0..=0xD8).contains(&marker) {
            continue;
        }
        if pos + 2 > data.len() {
            break;
        }
        let seg_len = u16::from_be_bytes([data[pos], data[pos + 1]]) as usize;
        if seg_len < 2 || pos + seg_len > data.len() {
            break;
        }
        let seg = &data[pos + 2..pos + seg_len];
        pos += seg_len;

        match marker {
            0xDB => {
                // DQT
                let mut i = 0;
                while i < seg.len() {
                    let pq_tq = seg[i];
                    i += 1;
                    let prec = (pq_tq >> 4) & 0x0F;
                    let tq = (pq_tq & 0x0F) as usize;
                    if tq >= 4 {
                        break;
                    }
                    if prec == 0 && i + 64 <= seg.len() {
                        for k in 0..64 {
                            qtables[tq][ZIGZAG[k]] = seg[i + k] as i32;
                        }
                        i += 64;
                    } else if prec == 1 && i + 128 <= seg.len() {
                        for k in 0..64 {
                            qtables[tq][ZIGZAG[k]] = u16::from_be_bytes([seg[i + k * 2], seg[i + k * 2 + 1]]) as i32;
                        }
                        i += 128;
                    } else {
                        break;
                    }
                }
            }
            0xC0 => {
                // SOF0 Baseline DCT
                if seg.len() < 6 {
                    return None;
                }
                height = u16::from_be_bytes([seg[1], seg[2]]) as usize;
                width = u16::from_be_bytes([seg[3], seg[4]]) as usize;
                let ncomp = seg[5] as usize;
                if width == 0 || height == 0 || width > 4096 || height > 4096 || seg.len() < 6 + ncomp * 3 {
                    return None;
                }
                comps.clear();
                for c in 0..ncomp {
                    let off = 6 + c * 3;
                    let id = seg[off];
                    let hv = seg[off + 1];
                    let qt = (seg[off + 2] as usize).min(3);
                    comps.push(ComponentInfo {
                        id,
                        h_samp: ((hv >> 4) & 0x0F).max(1) as usize,
                        v_samp: (hv & 0x0F).max(1) as usize,
                        qt_id: qt,
                        ..Default::default()
                    });
                }
            }
            0xC2 => {
                // Progressive JPEG not supported in lightweight preview decoder
                return None;
            }
            0xC4 => {
                // DHT
                let mut i = 0;
                while i + 17 <= seg.len() {
                    let tc_th = seg[i];
                    let tc = (tc_th >> 4) & 0x0F;
                    let th = (tc_th & 0x0F) as usize;
                    let mut counts = [0u8; 16];
                    counts.copy_from_slice(&seg[i + 1..i + 17]);
                    let total: usize = counts.iter().map(|&c| c as usize).sum();
                    i += 17;
                    if i + total > seg.len() {
                        break;
                    }
                    let table = HuffTable::build(&counts, &seg[i..i + total]);
                    i += total;
                    if th < 4 {
                        if tc == 0 {
                            dc_tables[th] = table;
                        } else {
                            ac_tables[th] = table;
                        }
                    }
                }
            }
            0xDD => {
                if seg.len() >= 2 {
                    restart_interval = u16::from_be_bytes([seg[0], seg[1]]) as usize;
                }
            }
            0xDA => {
                // SOS
                if seg.is_empty() {
                    return None;
                }
                let ns = seg[0] as usize;
                if seg.len() < 1 + ns * 2 {
                    return None;
                }
                for c in 0..ns {
                    let cid = seg[1 + c * 2];
                    let td_ta = seg[2 + c * 2];
                    if let Some(comp) = comps.iter_mut().find(|x| x.id == cid) {
                        comp.dc_id = ((td_ta >> 4) & 0x0F).min(3) as usize;
                        comp.ac_id = (td_ta & 0x0F).min(3) as usize;
                    }
                }
                scan_start = pos;
                break;
            }
            _ => {}
        }
    }

    if width == 0 || height == 0 || comps.is_empty() || scan_start == 0 {
        return None;
    }

    let max_h = comps.iter().map(|c| c.h_samp).max().unwrap_or(1);
    let max_v = comps.iter().map(|c| c.v_samp).max().unwrap_or(1);
    let mcu_w = max_h * 8;
    let mcu_h = max_v * 8;
    let mcu_cols = (width + mcu_w - 1) / mcu_w;
    let mcu_rows = (height + mcu_h - 1) / mcu_h;

    let mut pixels = vec![0xFF000000u32; width * height];
    let mut reader = BitReader::new(&data[scan_start..]);
    let mut mcu_idx = 0usize;

    // Temporary MCU component storage
    let mut comp_bufs: Vec<Vec<u8>> = comps
        .iter()
        .map(|c| vec![128u8; (c.h_samp * 8) * (c.v_samp * 8)])
        .collect();

    for my in 0..mcu_rows {
        for mx in 0..mcu_cols {
            if restart_interval > 0 && mcu_idx > 0 && (mcu_idx % restart_interval) == 0 {
                reader.align_byte();
                for c in &mut comps {
                    c.dc_pred = 0;
                }
            }
            mcu_idx += 1;

            for (ci, comp) in comps.iter_mut().enumerate() {
                let bw = comp.h_samp * 8;
                for bv in 0..comp.v_samp {
                    for bh in 0..comp.h_samp {
                        let mut block = [0i32; 64];
                        let dc_cat = reader.decode_huff(&dc_tables[comp.dc_id])?;
                        let dc_diff = receive_extend(&mut reader, dc_cat & 0x0F)?;
                        comp.dc_pred += dc_diff;
                        block[0] = comp.dc_pred * qtables[comp.qt_id][0];

                        let mut k = 1usize;
                        while k < 64 {
                            let sym = reader.decode_huff(&ac_tables[comp.ac_id])?;
                            if sym == 0 {
                                break;
                            }
                            let run = (sym >> 4) as usize;
                            let size = sym & 0x0F;
                            k += run;
                            if k >= 64 {
                                break;
                            }
                            let val = receive_extend(&mut reader, size)?;
                            let zz = ZIGZAG[k];
                            block[zz] = val * qtables[comp.qt_id][zz];
                            k += 1;
                        }

                        let spatial = idct_8x8(&block);
                        for py in 0..8 {
                            for px in 0..8 {
                                let dst_x = bh * 8 + px;
                                let dst_y = bv * 8 + py;
                                comp_bufs[ci][dst_y * bw + dst_x] = spatial[py * 8 + px];
                            }
                        }
                    }
                }
            }

            // Convert MCU to RGB pixels
            for y in 0..mcu_h {
                let gy = my * mcu_h + y;
                if gy >= height {
                    continue;
                }
                for x in 0..mcu_w {
                    let gx = mx * mcu_w + x;
                    if gx >= width {
                        continue;
                    }
                    let rgb = if comps.len() >= 3 {
                        let y_val = sample_comp(&comp_bufs[0], comps[0].h_samp, comps[0].v_samp, max_h, max_v, x, y) as f32;
                        let cb = sample_comp(&comp_bufs[1], comps[1].h_samp, comps[1].v_samp, max_h, max_v, x, y) as f32 - 128.0;
                        let cr = sample_comp(&comp_bufs[2], comps[2].h_samp, comps[2].v_samp, max_h, max_v, x, y) as f32 - 128.0;
                        let r = (y_val + 1.402 * cr).clamp(0.0, 255.0) as u32;
                        let g = (y_val - 0.344136 * cb - 0.714136 * cr).clamp(0.0, 255.0) as u32;
                        let b = (y_val + 1.772 * cb).clamp(0.0, 255.0) as u32;
                        0xFF000000 | (r << 16) | (g << 8) | b
                    } else {
                        let g = sample_comp(&comp_bufs[0], comps[0].h_samp, comps[0].v_samp, max_h, max_v, x, y) as u32;
                        0xFF000000 | (g << 16) | (g << 8) | g
                    };
                    pixels[gy * width + gx] = rgb;
                }
            }
        }
    }

    Some(DecodedImage {
        width,
        height,
        pixels,
    })
}

fn sample_comp(buf: &[u8], h: usize, v: usize, max_h: usize, max_v: usize, x: usize, y: usize) -> u8 {
    let sx = (x * h) / max_h;
    let sy = (y * v) / max_v;
    let bw = h * 8;
    buf[sy * bw + sx]
}

fn idct_8x8(block: &[i32; 64]) -> [u8; 64] {
    let mut temp = [0.0f32; 64];
    const C: [f32; 8] = [0.70710678, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0];

    for y in 0..8 {
        for x in 0..8 {
            let mut sum = 0.0f32;
            for u in 0..8 {
                let angle = ((2 * x + 1) * u) as f32 * std::f32::consts::PI / 16.0;
                sum += C[u] * (block[y * 8 + u] as f32) * angle.cos();
            }
            temp[y * 8 + x] = sum * 0.5;
        }
    }

    let mut out = [0u8; 64];
    for x in 0..8 {
        for y in 0..8 {
            let mut sum = 0.0f32;
            for v in 0..8 {
                let angle = ((2 * y + 1) * v) as f32 * std::f32::consts::PI / 16.0;
                sum += C[v] * temp[v * 8 + x] * angle.cos();
            }
            out[y * 8 + x] = (sum * 0.5 + 128.0).clamp(0.0, 255.0) as u8;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decode_tiny_png() {
        // Build a minimal 2x2 RGBA PNG via zlib_compress
        let mut raw = Vec::new();
        // row 0: filter 0, red, green
        raw.extend_from_slice(&[0, 255, 0, 0, 255, 0, 255, 0, 255]);
        // row 1: filter 0, blue, white
        raw.extend_from_slice(&[0, 0, 0, 255, 255, 255, 255, 255, 255]);
        let compressed = miniz_oxide::deflate::compress_to_vec_zlib(&raw, 1);

        let mut png = Vec::new();
        png.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&2u32.to_be_bytes());
        ihdr.extend_from_slice(&2u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        push_chunk(&mut png, b"IHDR", &ihdr);
        push_chunk(&mut png, b"IDAT", &compressed);
        push_chunk(&mut png, b"IEND", &[]);

        let img = decode_image(&png).unwrap();
        assert_eq!(img.width, 2);
        assert_eq!(img.height, 2);
        assert_eq!(img.pixels[0], 0xFFFF0000);
        assert_eq!(img.pixels[1], 0xFF00FF00);
        assert_eq!(img.pixels[2], 0xFF0000FF);
        assert_eq!(img.pixels[3], 0xFFFFFFFF);
    }

    fn push_chunk(out: &mut Vec<u8>, ctype: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(ctype);
        out.extend_from_slice(data);
        out.extend_from_slice(&0u32.to_be_bytes()); // CRC ignored by decoder
    }
}
