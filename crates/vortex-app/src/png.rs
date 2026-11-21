//! vortex-app/src/png.rs
//! Pure Rust uncompressed/deflate PNG image encoder with zero external dependencies.

pub fn encode_png_argb(width: u32, height: u32, pixels: &[u32]) -> Vec<u8> {
    let mut out = Vec::new();
    // 1. PNG Signature
    out.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);

    // 2. IHDR Chunk
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.push(8); // Bit depth: 8
    ihdr.push(6); // Color type: 6 (RGBA)
    ihdr.push(0); // Compression method: deflate
    ihdr.push(0); // Filter method: standard
    ihdr.push(0); // Interlace method: none
    write_chunk(&mut out, b"IHDR", &ihdr);

    // 3. Raw Scanlines (Filter type 0 + RGBA bytes)
    let row_bytes = (width as usize) * 4;
    let mut raw_data = Vec::with_capacity((row_bytes + 1) * (height as usize));
    for y in 0..(height as usize) {
        raw_data.push(0); // Filter byte: None
        for x in 0..(width as usize) {
            let px = pixels[y * (width as usize) + x];
            let a = ((px >> 24) & 0xFF) as u8;
            let r = ((px >> 16) & 0xFF) as u8;
            let g = ((px >> 8) & 0xFF) as u8;
            let b = (px & 0xFF) as u8;
            raw_data.push(r);
            raw_data.push(g);
            raw_data.push(b);
            raw_data.push(a);
        }
    }

    // 4. Deflate zlib stream (RFC 1950)
    let compressed = zlib_compress(&raw_data);
    write_chunk(&mut out, b"IDAT", &compressed);

    // 5. IEND Chunk
    write_chunk(&mut out, b"IEND", &[]);

    out
}

fn write_chunk(out: &mut Vec<u8>, chunk_type: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(chunk_type);
    out.extend_from_slice(data);

    let mut crc = Crc32::new();
    crc.update(chunk_type);
    crc.update(data);
    out.extend_from_slice(&crc.finalize().to_be_bytes());
}

fn zlib_compress(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(0x78); // Deflate, 32K window
    out.push(0x01); // No compression preset

    // Store in uncompressed blocks of up to 65535 bytes
    let mut offset = 0;
    while offset < data.len() {
        let chunk_len = (data.len() - offset).min(65535);
        let is_last = offset + chunk_len >= data.len();

        out.push(if is_last { 0x01 } else { 0x00 });
        let len_u16 = chunk_len as u16;
        let nlen_u16 = !len_u16;

        out.extend_from_slice(&len_u16.to_le_bytes());
        out.extend_from_slice(&nlen_u16.to_le_bytes());
        out.extend_from_slice(&data[offset..offset + chunk_len]);

        offset += chunk_len;
    }

    // Adler-32 Checksum
    let mut s1: u32 = 1;
    let mut s2: u32 = 0;
    for &b in data {
        s1 = (s1 + b as u32) % 65521;
        s2 = (s2 + s1) % 65521;
    }
    let adler = (s2 << 16) | s1;
    out.extend_from_slice(&adler.to_be_bytes());

    out
}

struct Crc32(u32);

impl Crc32 {
    fn new() -> Self {
        Self(0xFFFFFFFF)
    }

    fn update(&mut self, data: &[u8]) {
        for &b in data {
            let mut c = (self.0 ^ (b as u32)) & 0xFF;
            for _ in 0..8 {
                if (c & 1) != 0 {
                    c = 0xEDB88320 ^ (c >> 1);
                } else {
                    c >>= 1;
                }
            }
            self.0 = c ^ (self.0 >> 8);
        }
    }

    fn finalize(self) -> u32 {
        !self.0
    }
}
