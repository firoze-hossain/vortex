use std::cmp::Ordering;
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, Copy)]
struct BoxHeader {
    offset: u64,
    size: u64,
    header_size: u64,
    typ: [u8; 4],
}

#[derive(Debug, Clone)]
struct SampleRef {
    file_offset: u64,
    size: u32,
    duration: u32,
    cto: i32,
    is_sync: bool,
}

#[derive(Debug, Clone)]
struct TrackSource {
    track_id: u32,
    timescale: u32,
    tkhd: Vec<u8>,
    edts: Option<Vec<u8>>,
    mdhd: Vec<u8>,
    hdlr: Vec<u8>,
    minf_other_children: Vec<Vec<u8>>,
    stsd: Vec<u8>,
    samples: Vec<SampleRef>,
    total_duration: u64,
}

#[derive(Debug, Clone)]
struct MediaChunk {
    track_index: usize, // 0 = video, 1 = audio
    start_ns: u128,
    samples: Vec<SampleRef>,
}

fn read_be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

fn read_be32_signed(b: &[u8]) -> i32 {
    i32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

fn read_be64(b: &[u8]) -> u64 {
    u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
}

fn write_be32(b: &mut [u8], v: u32) {
    b.copy_from_slice(&v.to_be_bytes());
}

fn write_be64(b: &mut [u8], v: u64) {
    b.copy_from_slice(&v.to_be_bytes());
}

fn make_box(typ: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + payload.len());
    out.extend_from_slice(&((8 + payload.len()) as u32).to_be_bytes());
    out.extend_from_slice(typ);
    out.extend_from_slice(payload);
    out
}

fn make_full_box(typ: &[u8; 4], version: u8, flags: u32, payload: &[u8]) -> Vec<u8> {
    let vf = ((version as u32) << 24) | (flags & 0x00FF_FFFF);
    let mut full = Vec::with_capacity(4 + payload.len());
    full.extend_from_slice(&vf.to_be_bytes());
    full.extend_from_slice(payload);
    make_box(typ, &full)
}

fn scan_file_top_boxes(file: &mut File) -> Result<Vec<BoxHeader>, String> {
    let file_len = file
        .seek(SeekFrom::End(0))
        .map_err(|e| format!("MP4 seek end failed: {}", e))?;
    let mut pos = 0u64;
    let mut out = Vec::new();
    let mut hdr = [0u8; 8];

    while pos + 8 <= file_len {
        file.seek(SeekFrom::Start(pos))
            .map_err(|e| format!("MP4 seek failed at {}: {}", pos, e))?;
        file.read_exact(&mut hdr)
            .map_err(|e| format!("MP4 header read failed at {}: {}", pos, e))?;
        let size32 = read_be32(&hdr[0..4]) as u64;
        let typ = [hdr[4], hdr[5], hdr[6], hdr[7]];
        let (size, header_size) = if size32 == 1 {
            if pos + 16 > file_len {
                break;
            }
            let mut ext = [0u8; 8];
            file.read_exact(&mut ext)
                .map_err(|e| format!("MP4 64-bit size read failed: {}", e))?;
            (read_be64(&ext), 16u64)
        } else if size32 == 0 {
            (file_len - pos, 8u64)
        } else {
            (size32, 8u64)
        };

        if size < header_size || pos + size > file_len {
            return Err(format!(
                "Invalid top-level box {:?} size {} at {} (len {})",
                std::str::from_utf8(&typ).unwrap_or("????"),
                size,
                pos,
                file_len
            ));
        }

        out.push(BoxHeader {
            offset: pos,
            size,
            header_size,
            typ,
        });
        pos += size;
    }

    Ok(out)
}

fn read_box_bytes(file: &mut File, b: BoxHeader) -> Result<Vec<u8>, String> {
    if b.size > 64 * 1024 * 1024 {
        return Err("MP4 metadata box exceeds 64 MB limit".into());
    }
    let mut buf = vec![0u8; b.size as usize];
    file.seek(SeekFrom::Start(b.offset))
        .map_err(|e| format!("MP4 read seek failed: {}", e))?;
    file.read_exact(&mut buf)
        .map_err(|e| format!("MP4 read box failed: {}", e))?;
    Ok(buf)
}

/// Iterates over child boxes inside a parent box's payload slice.
/// Yields `(box_type, full_child_slice, header_len)`.
fn split_mem_boxes(slice: &[u8]) -> Vec<([u8; 4], &[u8], usize)> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos + 8 <= slice.len() {
        let size32 = read_be32(&slice[pos..pos + 4]) as usize;
        let typ = [slice[pos + 4], slice[pos + 5], slice[pos + 6], slice[pos + 7]];
        let (size, hdr) = if size32 == 1 && pos + 16 <= slice.len() {
            (read_be64(&slice[pos + 8..pos + 16]) as usize, 16usize)
        } else if size32 == 0 {
            (slice.len() - pos, 8usize)
        } else {
            (size32, 8usize)
        };
        if size < hdr || pos + size > slice.len() {
            break;
        }
        out.push((typ, &slice[pos..pos + size], hdr));
        pos += size;
    }
    out
}

fn parse_tkhd_track_id(tkhd: &[u8]) -> Option<u32> {
    let hdr = if tkhd.len() >= 16 && read_be32(&tkhd[0..4]) == 1 { 16 } else { 8 };
    if tkhd.len() < hdr + 16 {
        return None;
    }
    let version = tkhd[hdr];
    let id_off = if version == 1 { hdr + 20 } else { hdr + 12 };
    if tkhd.len() < id_off + 4 {
        return None;
    }
    Some(read_be32(&tkhd[id_off..id_off + 4]))
}

fn parse_mdhd_timescale(mdhd: &[u8]) -> Option<u32> {
    let hdr = if mdhd.len() >= 16 && read_be32(&mdhd[0..4]) == 1 { 16 } else { 8 };
    if mdhd.len() < hdr + 16 {
        return None;
    }
    let version = mdhd[hdr];
    let scale_off = if version == 1 { hdr + 20 } else { hdr + 12 };
    if mdhd.len() < scale_off + 4 {
        return None;
    }
    let ts = read_be32(&mdhd[scale_off..scale_off + 4]);
    (ts > 0).then_some(ts)
}

fn parse_hdlr_type(hdlr: &[u8]) -> Option<[u8; 4]> {
    let hdr = if hdlr.len() >= 16 && read_be32(&hdlr[0..4]) == 1 { 16 } else { 8 };
    if hdlr.len() < hdr + 12 {
        return None;
    }
    let off = hdr + 8;
    Some([hdlr[off], hdlr[off + 1], hdlr[off + 2], hdlr[off + 3]])
}

fn parse_trex_defaults(moov: &[u8], mvex_slice_hdr: usize) -> Vec<(u32, u32, u32, u32)> {
    let mut out = Vec::new();
    for (t, child, chdr) in split_mem_boxes(&moov[mvex_slice_hdr..]) {
        if &t == b"trex" && child.len() >= chdr + 24 {
            let b = &child[chdr..];
            let tid = read_be32(&b[4..8]);
            let dd = read_be32(&b[12..16]);
            let ds = read_be32(&b[16..20]);
            let df = read_be32(&b[20..24]);
            out.push((tid, dd, ds, df));
        }
    }
    out
}

fn extract_stbl_samples(stbl: &[u8], stbl_hdr: usize) -> Result<Vec<SampleRef>, String> {
    let mut stts_entries: Vec<(u32, u32)> = Vec::new();
    let mut ctts_entries: Vec<(u32, i32)> = Vec::new();
    let mut sync_set: Option<Vec<u32>> = None;
    let mut stsc_entries: Vec<(u32, u32)> = Vec::new();
    let mut sample_sizes: Vec<u32> = Vec::new();
    let mut chunk_offsets: Vec<u64> = Vec::new();

    for (typ, box_bytes, hdr) in split_mem_boxes(&stbl[stbl_hdr..]) {
        let pay = &box_bytes[hdr..];
        match &typ {
            b"stts" if pay.len() >= 8 => {
                let count = read_be32(&pay[4..8]) as usize;
                for i in 0..count {
                    let o = 8 + i * 8;
                    if o + 8 <= pay.len() {
                        stts_entries.push((read_be32(&pay[o..o + 4]), read_be32(&pay[o + 4..o + 8])));
                    }
                }
            }
            b"ctts" if pay.len() >= 8 => {
                let ver = pay[0];
                let count = read_be32(&pay[4..8]) as usize;
                for i in 0..count {
                    let o = 8 + i * 8;
                    if o + 8 <= pay.len() {
                        let c = read_be32(&pay[o..o + 4]);
                        let off = if ver == 1 {
                            read_be32_signed(&pay[o + 4..o + 8])
                        } else {
                            read_be32(&pay[o + 4..o + 8]) as i32
                        };
                        ctts_entries.push((c, off));
                    }
                }
            }
            b"stss" if pay.len() >= 8 => {
                let count = read_be32(&pay[4..8]) as usize;
                let mut v = Vec::with_capacity(count);
                for i in 0..count {
                    let o = 8 + i * 4;
                    if o + 4 <= pay.len() {
                        v.push(read_be32(&pay[o..o + 4]));
                    }
                }
                sync_set = Some(v);
            }
            b"stsc" if pay.len() >= 8 => {
                let count = read_be32(&pay[4..8]) as usize;
                for i in 0..count {
                    let o = 8 + i * 12;
                    if o + 12 <= pay.len() {
                        stsc_entries.push((read_be32(&pay[o..o + 4]), read_be32(&pay[o + 4..o + 8])));
                    }
                }
            }
            b"stsz" if pay.len() >= 12 => {
                let fixed_size = read_be32(&pay[4..8]);
                let count = read_be32(&pay[8..12]) as usize;
                if fixed_size > 0 {
                    sample_sizes = vec![fixed_size; count];
                } else {
                    sample_sizes.reserve(count);
                    for i in 0..count {
                        let o = 12 + i * 4;
                        if o + 4 <= pay.len() {
                            sample_sizes.push(read_be32(&pay[o..o + 4]));
                        }
                    }
                }
            }
            b"stco" if pay.len() >= 8 => {
                let count = read_be32(&pay[4..8]) as usize;
                chunk_offsets.reserve(count);
                for i in 0..count {
                    let o = 8 + i * 4;
                    if o + 4 <= pay.len() {
                        chunk_offsets.push(read_be32(&pay[o..o + 4]) as u64);
                    }
                }
            }
            b"co64" if pay.len() >= 8 => {
                let count = read_be32(&pay[4..8]) as usize;
                chunk_offsets.reserve(count);
                for i in 0..count {
                    let o = 8 + i * 8;
                    if o + 8 <= pay.len() {
                        chunk_offsets.push(read_be64(&pay[o..o + 8]));
                    }
                }
            }
            _ => {}
        }
    }

    if sample_sizes.is_empty() || chunk_offsets.is_empty() || stsc_entries.is_empty() {
        return Ok(Vec::new());
    }

    // Expand sample durations and composition offsets
    let mut durations = Vec::with_capacity(sample_sizes.len());
    for (c, d) in stts_entries {
        for _ in 0..c {
            if durations.len() < sample_sizes.len() {
                durations.push(d);
            }
        }
    }
    let mut ctos = Vec::with_capacity(sample_sizes.len());
    for (c, off) in ctts_entries {
        for _ in 0..c {
            if ctos.len() < sample_sizes.len() {
                ctos.push(off);
            }
        }
    }

    let mut samples = Vec::with_capacity(sample_sizes.len());
    let mut s_idx = 0usize;
    let mut sync_cursor = 0usize;
    let total_chunks = chunk_offsets.len();

    for ci in 0..total_chunks {
        let chunk_num = (ci + 1) as u32;
        let mut spc = stsc_entries[0].1;
        for &(first_c, count) in &stsc_entries {
            if chunk_num >= first_c {
                spc = count;
            } else {
                break;
            }
        }
        let mut cur_off = chunk_offsets[ci];
        for _ in 0..spc {
            if s_idx >= sample_sizes.len() {
                break;
            }
            let sz = sample_sizes[s_idx];
            let dur = durations.get(s_idx).copied().unwrap_or(1);
            let cto = ctos.get(s_idx).copied().unwrap_or(0);
            let is_sync = if let Some(ref sset) = sync_set {
                while sync_cursor < sset.len() && sset[sync_cursor] < (s_idx as u32 + 1) {
                    sync_cursor += 1;
                }
                sync_cursor < sset.len() && sset[sync_cursor] == (s_idx as u32 + 1)
            } else {
                true
            };
            samples.push(SampleRef {
                file_offset: cur_off,
                size: sz,
                duration: dur,
                cto,
                is_sync,
            });
            cur_off += sz as u64;
            s_idx += 1;
        }
    }

    Ok(samples)
}

fn extract_fragments_samples(
    file: &mut File,
    top_boxes: &[BoxHeader],
    target_tid: u32,
    trex_default: (u32, u32, u32),
) -> Result<Vec<SampleRef>, String> {
    let mut samples = Vec::new();
    let mut pending_moof: Option<BoxHeader> = None;

    for &b in top_boxes {
        match &b.typ {
            b"moof" => pending_moof = Some(b),
            b"mdat" => {
                let Some(moof_hdr) = pending_moof.take() else {
                    continue;
                };
                let moof_bytes = read_box_bytes(file, moof_hdr)?;
                let mdat_data_start = b.offset + b.header_size;
                let mdat_data_end = b.offset + b.size;
                let moof_payload_start = moof_hdr.header_size as usize;

                for (ct, cc, chdr) in split_mem_boxes(&moof_bytes[moof_payload_start..]) {
                    if &ct != b"traf" {
                        continue;
                    }
                    let mut tid = 0u32;
                    let mut tf_flags = 0u32;
                    let mut base_data_offset = moof_hdr.offset;
                    let (mut def_dur, mut def_size, mut def_flags) = trex_default;
                    let mut truns: Vec<&[u8]> = Vec::new();

                    for (tt, tc, thdr) in split_mem_boxes(&cc[chdr..]) {
                        let pay = &tc[thdr..];
                        if &tt == b"tfhd" && pay.len() >= 8 {
                            tf_flags = read_be32(&pay[0..4]) & 0x00FF_FFFF;
                            tid = read_be32(&pay[4..8]);
                            let mut cur = 8usize;
                            if tf_flags & 0x0001 != 0 && cur + 8 <= pay.len() {
                                base_data_offset = read_be64(&pay[cur..cur + 8]);
                                cur += 8;
                            }
                            if tf_flags & 0x0002 != 0 && cur + 4 <= pay.len() {
                                cur += 4;
                            }
                            if tf_flags & 0x0008 != 0 && cur + 4 <= pay.len() {
                                def_dur = read_be32(&pay[cur..cur + 4]);
                                cur += 4;
                            }
                            if tf_flags & 0x0010 != 0 && cur + 4 <= pay.len() {
                                def_size = read_be32(&pay[cur..cur + 4]);
                                cur += 4;
                            }
                            if tf_flags & 0x0020 != 0 && cur + 4 <= pay.len() {
                                def_flags = read_be32(&pay[cur..cur + 4]);
                            }
                        } else if &tt == b"trun" {
                            truns.push(pay);
                        }
                    }

                    if tid != target_tid {
                        continue;
                    }

                    let mut cursor = mdat_data_start;
                    for pay in truns {
                        if pay.len() < 8 {
                            continue;
                        }
                        let tr_flags = read_be32(&pay[0..4]) & 0x00FF_FFFF;
                        let count = read_be32(&pay[4..8]) as usize;
                        let mut cur = 8usize;

                        if tr_flags & 0x0001 != 0 && cur + 4 <= pay.len() {
                            let data_offset = read_be32_signed(&pay[cur..cur + 4]) as i128;
                            let abs = (base_data_offset as i128 + data_offset).max(0) as u64;
                            cursor = abs;
                            cur += 4;
                        }

                        let first_flags = if tr_flags & 0x0004 != 0 && cur + 4 <= pay.len() {
                            let v = read_be32(&pay[cur..cur + 4]);
                            cur += 4;
                            v
                        } else {
                            def_flags
                        };

                        for idx in 0..count {
                            let dur = if tr_flags & 0x0100 != 0 && cur + 4 <= pay.len() {
                                let v = read_be32(&pay[cur..cur + 4]);
                                cur += 4;
                                v
                            } else {
                                def_dur
                            };
                            let sz = if tr_flags & 0x0200 != 0 && cur + 4 <= pay.len() {
                                let v = read_be32(&pay[cur..cur + 4]);
                                cur += 4;
                                v
                            } else {
                                def_size
                            };
                            let (sflags, has_flags) = if tr_flags & 0x0400 != 0 && cur + 4 <= pay.len() {
                                let v = read_be32(&pay[cur..cur + 4]);
                                cur += 4;
                                (v, true)
                            } else if idx == 0 && (tr_flags & 0x0004 != 0) {
                                (first_flags, true)
                            } else if tf_flags & 0x0020 != 0 {
                                (def_flags, true)
                            } else {
                                (def_flags, trex_default.2 != 0)
                            };
                            let cto = if tr_flags & 0x0800 != 0 && cur + 4 <= pay.len() {
                                let v = read_be32_signed(&pay[cur..cur + 4]);
                                cur += 4;
                                v
                            } else {
                                0
                            };

                            let is_sync = if has_flags {
                                ((sflags >> 16) & 1) == 0
                            } else {
                                idx == 0
                            };

                            if sz > 0 && cursor >= mdat_data_start && cursor + sz as u64 <= mdat_data_end {
                                samples.push(SampleRef {
                                    file_offset: cursor,
                                    size: sz,
                                    duration: dur,
                                    cto,
                                    is_sync,
                                });
                            }
                            cursor += sz as u64;
                        }
                    }
                }
            }
            _ => {}
        }
    }

    Ok(samples)
}

fn load_track_source(
    file: &mut File,
    wanted_handler: &[u8; 4],
) -> Result<(u32, TrackSource), String> {
    let top = scan_file_top_boxes(file)?;
    let moov_hdr = top
        .iter()
        .find(|b| &b.typ == b"moov")
        .copied()
        .ok_or_else(|| "Missing moov box in MP4 stream".to_string())?;
    let moov_bytes = read_box_bytes(file, moov_hdr)?;
    let moov_pay = &moov_bytes[moov_hdr.header_size as usize..];

    let mut mvhd_timescale = 1000u32;
    let mut trex_list: Vec<(u32, u32, u32, u32)> = Vec::new();
    let mut candidates: Vec<(TrackSource, Option<&[u8]>, usize, [u8; 4])> = Vec::new();

    for (typ, slice, hdr) in split_mem_boxes(moov_pay) {
        if &typ == b"mvhd" && slice.len() >= hdr + 24 {
            let ver = slice[hdr];
            let ts_off = if ver == 1 { hdr + 20 } else { hdr + 12 };
            if slice.len() >= ts_off + 4 {
                let ts = read_be32(&slice[ts_off..ts_off + 4]);
                if ts > 0 {
                    mvhd_timescale = ts;
                }
            }
        } else if &typ == b"mvex" {
            let slice_offset_in_moov = slice.as_ptr() as usize - moov_bytes.as_ptr() as usize;
            trex_list = parse_trex_defaults(&moov_bytes, slice_offset_in_moov + hdr);
        } else if &typ == b"trak" {
            let mut tkhd = None;
            let mut edts = None;
            let mut mdhd = None;
            let mut hdlr = None;
            let mut hdlr_kind = [0u8; 4];
            let mut timescale = 0u32;
            let mut track_id = 0u32;
            let mut minf_other = Vec::new();
            let mut stsd = None;
            let mut stbl_slice_info: Option<(usize, usize)> = None;

            for (tt, ts, th) in split_mem_boxes(&slice[hdr..]) {
                match &tt {
                    b"tkhd" => {
                        track_id = parse_tkhd_track_id(ts).unwrap_or(0);
                        tkhd = Some(ts.to_vec());
                    }
                    b"edts" => {
                        edts = Some(ts.to_vec());
                    }
                    b"mdia" => {
                        for (mt, ms, mh) in split_mem_boxes(&ts[mh_of(th, ts)..]) {
                            let _ = mh;
                            match &mt {
                                b"mdhd" => {
                                    timescale = parse_mdhd_timescale(ms).unwrap_or(0);
                                    mdhd = Some(ms.to_vec());
                                }
                                b"hdlr" => {
                                    hdlr_kind = parse_hdlr_type(ms).unwrap_or([0; 4]);
                                    hdlr = Some(ms.to_vec());
                                }
                                b"minf" => {
                                    for (nt, ns, nh) in split_mem_boxes(&ms[mh..]) {
                                        if &nt == b"stbl" {
                                            let off = ns.as_ptr() as usize - moov_bytes.as_ptr() as usize;
                                            stbl_slice_info = Some((off, nh));
                                            for (st, ss, _) in split_mem_boxes(&ns[nh..]) {
                                                if &st == b"stsd" {
                                                    stsd = Some(ss.to_vec());
                                                }
                                            }
                                        } else {
                                            minf_other.push(ns.to_vec());
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }

            if track_id > 0 && timescale > 0 {
                if let (Some(tkhd), Some(mdhd), Some(hdlr), Some(stsd)) = (tkhd, mdhd, hdlr, stsd) {
                    let src = TrackSource {
                        track_id,
                        timescale,
                        tkhd,
                        edts,
                        mdhd,
                        hdlr,
                        minf_other_children: minf_other,
                        stsd,
                        samples: Vec::new(),
                        total_duration: 0,
                    };
                    let stbl_ref = stbl_slice_info.map(|(off, _)| &moov_bytes[off..]);
                    let stbl_hdr = stbl_slice_info.map(|(_, h)| h).unwrap_or(8);
                    candidates.push((src, stbl_ref, stbl_hdr, hdlr_kind));
                }
            }
        }
    }

    let chosen_idx = candidates
        .iter()
        .position(|(_, _, _, kind)| kind == wanted_handler)
        .or_else(|| (!candidates.is_empty()).then_some(0))
        .ok_or_else(|| "No valid media track found inside MP4".to_string())?;

    let (mut track, stbl_range_maybe, stbl_hdr, _) = candidates.remove(chosen_idx);
    let _ = stbl_range_maybe;

    let has_moof = top.iter().any(|b| &b.typ == b"moof");
    let trex_default = trex_list
        .iter()
        .find(|x| x.0 == track.track_id)
        .map(|x| (x.1, x.2, x.3))
        .unwrap_or((0, 0, 0));

    let samples = if has_moof {
        extract_fragments_samples(file, &top, track.track_id, trex_default)?
    } else {
        // Re-locate exact stbl slice from moov_bytes for chosen track_id
        let mut stbl_samples = Vec::new();
        for (typ, slice, hdr) in split_mem_boxes(moov_pay) {
            if &typ == b"trak" {
                let mut tid = 0u32;
                for (tt, ts, th) in split_mem_boxes(&slice[hdr..]) {
                    if &tt == b"tkhd" {
                        tid = parse_tkhd_track_id(ts).unwrap_or(0);
                    } else if &tt == b"mdia" && tid == track.track_id {
                        for (mt, ms, mh) in split_mem_boxes(&ts[th..]) {
                            if &mt == b"minf" {
                                for (nt, ns, nh) in split_mem_boxes(&ms[mh..]) {
                                    if &nt == b"stbl" {
                                        stbl_samples = extract_stbl_samples(ns, nh)?;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        let _ = stbl_hdr;
        stbl_samples
    };

    if samples.is_empty() {
        return Err("MP4 track contains 0 media samples".into());
    }

    track.total_duration = samples.iter().map(|s| s.duration as u64).sum();
    track.samples = samples;
    Ok((mvhd_timescale, track))
}

#[inline]
fn mh_of(th: usize, _ts: &[u8]) -> usize {
    th
}

fn group_into_chunks(track_index: usize, track: &TrackSource) -> Vec<MediaChunk> {
    let target_ticks = (track.timescale as u64 / 2).max(1);
    let mut chunks = Vec::new();
    let mut cur_samples = Vec::new();
    let mut cur_start_ticks = 0u64;
    let mut elapsed_ticks = 0u64;

    for s in &track.samples {
        if !cur_samples.is_empty() && elapsed_ticks.saturating_sub(cur_start_ticks) >= target_ticks {
            let start_ns = (cur_start_ticks as u128 * 1_000_000_000u128) / track.timescale as u128;
            chunks.push(MediaChunk {
                track_index,
                start_ns,
                samples: std::mem::take(&mut cur_samples),
            });
            cur_start_ticks = elapsed_ticks;
        }
        cur_samples.push(s.clone());
        elapsed_ticks += s.duration as u64;
    }

    if !cur_samples.is_empty() {
        let start_ns = (cur_start_ticks as u128 * 1_000_000_000u128) / track.timescale as u128;
        chunks.push(MediaChunk {
            track_index,
            start_ns,
            samples: cur_samples,
        });
    }

    chunks
}

fn patch_tkhd(tkhd: &[u8], new_track_id: u32, dur_mvhd: u64) -> Vec<u8> {
    let hdr = if tkhd.len() >= 16 && read_be32(&tkhd[0..4]) == 1 { 16 } else { 8 };
    let pay = &tkhd[hdr..];
    if pay.len() < 4 {
        return tkhd.to_vec();
    }
    let ver = pay[0];
    let mut body = pay.to_vec();
    if ver == 1 && body.len() >= 36 {
        write_be32(&mut body[20..24], new_track_id);
        write_be64(&mut body[28..36], dur_mvhd);
    } else if ver == 0 && body.len() >= 24 {
        write_be32(&mut body[12..16], new_track_id);
        write_be32(&mut body[20..24], dur_mvhd.min(u32::MAX as u64) as u32);
    }
    make_box(b"tkhd", &body)
}

fn patch_mdhd(mdhd: &[u8], total_duration: u64) -> Vec<u8> {
    let hdr = if mdhd.len() >= 16 && read_be32(&mdhd[0..4]) == 1 { 16 } else { 8 };
    let pay = &mdhd[hdr..];
    if pay.len() < 24 {
        return mdhd.to_vec();
    }
    let ver = pay[0];
    let mut body = pay.to_vec();
    if ver == 1 && body.len() >= 32 {
        write_be64(&mut body[24..32], total_duration);
    } else if ver == 0 && body.len() >= 20 {
        write_be32(&mut body[16..20], total_duration.min(u32::MAX as u64) as u32);
    }
    make_box(b"mdhd", &body)
}

fn patch_edts(edts: &[u8], dur_mvhd: u64) -> Vec<u8> {
    let hdr = if edts.len() >= 16 && read_be32(&edts[0..4]) == 1 { 16 } else { 8 };
    let mut children = Vec::new();
    for (t, child, chdr) in split_mem_boxes(&edts[hdr..]) {
        if &t == b"elst" && child.len() >= chdr + 8 {
            let mut body = child[chdr..].to_vec();
            let ver = body[0];
            let entry_count = read_be32(&body[4..8]);
            if entry_count == 1 {
                if ver == 0 && body.len() >= 20 {
                    let media_time = read_be32_signed(&body[12..16]);
                    if media_time >= 0 {
                        write_be32(&mut body[8..12], dur_mvhd.min(u32::MAX as u64) as u32);
                    }
                } else if ver == 1 && body.len() >= 28 {
                    let media_time = i64::from_be_bytes([
                        body[16], body[17], body[18], body[19], body[20], body[21], body[22], body[23],
                    ]);
                    if media_time >= 0 {
                        write_be64(&mut body[8..16], dur_mvhd);
                    }
                }
            }
            children.extend_from_slice(&make_box(b"elst", &body));
        } else {
            children.extend_from_slice(child);
        }
    }
    make_box(b"edts", &children)
}

fn build_mvhd(timescale: u32, duration: u64) -> Vec<u8> {
    // Emit standard mvhd v0 unless duration exceeds u32::MAX
    let mut body = vec![0u8; 96];
    write_be32(&mut body[8..12], timescale);
    write_be32(&mut body[12..16], duration.min(u32::MAX as u64) as u32);
    write_be32(&mut body[16..20], 0x0001_0000); // rate 1.0
    body[20] = 0x01;
    body[21] = 0x00; // volume 1.0
    // identity matrix at offset 32..68
    write_be32(&mut body[32..36], 0x0001_0000);
    write_be32(&mut body[48..52], 0x0001_0000);
    write_be32(&mut body[64..68], 0x4000_0000);
    write_be32(&mut body[92..96], 3); // next_track_ID = 3
    make_full_box(b"mvhd", 0, 0, &body)
}

fn build_stbl(
    track: &TrackSource,
    is_video: bool,
    chunk_offsets: &[u64],
    samples_per_chunk: &[u32],
    use_co64: bool,
) -> Vec<u8> {
    let mut stbl_children = Vec::new();
    stbl_children.extend_from_slice(&track.stsd);

    // 1. stts
    let mut stts_runs: Vec<(u32, u32)> = Vec::new();
    for s in &track.samples {
        if let Some(last) = stts_runs.last_mut() {
            if last.1 == s.duration {
                last.0 += 1;
                continue;
            }
        }
        stts_runs.push((1, s.duration));
    }
    let mut stts_pay = Vec::with_capacity(4 + stts_runs.len() * 8);
    stts_pay.extend_from_slice(&(stts_runs.len() as u32).to_be_bytes());
    for (cnt, dur) in stts_runs {
        stts_pay.extend_from_slice(&cnt.to_be_bytes());
        stts_pay.extend_from_slice(&dur.to_be_bytes());
    }
    stbl_children.extend_from_slice(&make_full_box(b"stts", 0, 0, &stts_pay));

    // 2. ctts (only if non-zero offsets exist)
    if track.samples.iter().any(|s| s.cto != 0) {
        let mut ctts_runs: Vec<(u32, i32)> = Vec::new();
        let mut has_negative = false;
        for s in &track.samples {
            if s.cto < 0 {
                has_negative = true;
            }
            if let Some(last) = ctts_runs.last_mut() {
                if last.1 == s.cto {
                    last.0 += 1;
                    continue;
                }
            }
            ctts_runs.push((1, s.cto));
        }
        let mut ctts_pay = Vec::with_capacity(4 + ctts_runs.len() * 8);
        ctts_pay.extend_from_slice(&(ctts_runs.len() as u32).to_be_bytes());
        for (cnt, cto) in ctts_runs {
            ctts_pay.extend_from_slice(&cnt.to_be_bytes());
            ctts_pay.extend_from_slice(&cto.to_be_bytes());
        }
        stbl_children.extend_from_slice(&make_full_box(
            b"ctts",
            if has_negative { 1 } else { 0 },
            0,
            &ctts_pay,
        ));
    }

    // 3. stss (sync sample table for video tracks with non-key frames)
    if is_video {
        let syncs: Vec<u32> = track
            .samples
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.is_sync.then_some((i + 1) as u32))
            .collect();
        if !syncs.is_empty() && syncs.len() < track.samples.len() {
            let mut stss_pay = Vec::with_capacity(4 + syncs.len() * 4);
            stss_pay.extend_from_slice(&(syncs.len() as u32).to_be_bytes());
            for idx in syncs {
                stss_pay.extend_from_slice(&idx.to_be_bytes());
            }
            stbl_children.extend_from_slice(&make_full_box(b"stss", 0, 0, &stss_pay));
        }
    }

    // 4. stsc
    let mut stsc_runs: Vec<(u32, u32)> = Vec::new();
    for (ci, &spc) in samples_per_chunk.iter().enumerate() {
        if stsc_runs.last().map(|r| r.1) != Some(spc) {
            stsc_runs.push(((ci + 1) as u32, spc));
        }
    }
    let mut stsc_pay = Vec::with_capacity(4 + stsc_runs.len() * 12);
    stsc_pay.extend_from_slice(&(stsc_runs.len() as u32).to_be_bytes());
    for (first_c, spc) in stsc_runs {
        stsc_pay.extend_from_slice(&first_c.to_be_bytes());
        stsc_pay.extend_from_slice(&spc.to_be_bytes());
        stsc_pay.extend_from_slice(&1u32.to_be_bytes());
    }
    stbl_children.extend_from_slice(&make_full_box(b"stsc", 0, 0, &stsc_pay));

    // 5. stsz
    let mut stsz_pay = Vec::with_capacity(8 + track.samples.len() * 4);
    stsz_pay.extend_from_slice(&0u32.to_be_bytes());
    stsz_pay.extend_from_slice(&(track.samples.len() as u32).to_be_bytes());
    for s in &track.samples {
        stsz_pay.extend_from_slice(&s.size.to_be_bytes());
    }
    stbl_children.extend_from_slice(&make_full_box(b"stsz", 0, 0, &stsz_pay));

    // 6. stco or co64
    if use_co64 {
        let mut co64_pay = Vec::with_capacity(4 + chunk_offsets.len() * 8);
        co64_pay.extend_from_slice(&(chunk_offsets.len() as u32).to_be_bytes());
        for &off in chunk_offsets {
            co64_pay.extend_from_slice(&off.to_be_bytes());
        }
        stbl_children.extend_from_slice(&make_full_box(b"co64", 0, 0, &co64_pay));
    } else {
        let mut stco_pay = Vec::with_capacity(4 + chunk_offsets.len() * 4);
        stco_pay.extend_from_slice(&(chunk_offsets.len() as u32).to_be_bytes());
        for &off in chunk_offsets {
            stco_pay.extend_from_slice(&(off as u32).to_be_bytes());
        }
        stbl_children.extend_from_slice(&make_full_box(b"stco", 0, 0, &stco_pay));
    }

    make_box(b"stbl", &stbl_children)
}

fn build_trak_box(
    track: &TrackSource,
    new_track_id: u32,
    mvhd_timescale: u32,
    is_video: bool,
    chunk_offsets: &[u64],
    samples_per_chunk: &[u32],
    use_co64: bool,
) -> Vec<u8> {
    let dur_mvhd = (track.total_duration as u128 * mvhd_timescale as u128 / track.timescale as u128) as u64;
    let mut trak_children = Vec::new();
    trak_children.extend_from_slice(&patch_tkhd(&track.tkhd, new_track_id, dur_mvhd));
    if let Some(ref edts) = track.edts {
        trak_children.extend_from_slice(&patch_edts(edts, dur_mvhd));
    }

    let mut minf_children = Vec::new();
    for other in &track.minf_other_children {
        minf_children.extend_from_slice(other);
    }
    minf_children.extend_from_slice(&build_stbl(
        track,
        is_video,
        chunk_offsets,
        samples_per_chunk,
        use_co64,
    ));
    let minf = make_box(b"minf", &minf_children);

    let mut mdia_children = Vec::new();
    mdia_children.extend_from_slice(&patch_mdhd(&track.mdhd, track.total_duration));
    mdia_children.extend_from_slice(&track.hdlr);
    mdia_children.extend_from_slice(&minf);
    trak_children.extend_from_slice(&make_box(b"mdia", &mdia_children));

    make_box(b"trak", &trak_children)
}

/// Pure-Rust streaming MP4 remuxer: combines separate DASH/MP4 video + audio tracks
/// into a single standard progressive MP4 (`ftyp` + `moov` with full `stbl` + tightly
/// interleaved ~0.5s chunks inside one `mdat`), preventing player demux buffer deadlocks.
pub fn mux_fmp4_files(video_path: &Path, audio_path: &Path, output_path: &Path) -> Result<(), String> {
    let mut vf = File::open(video_path)
        .map_err(|e| format!("Failed opening video part {}: {}", video_path.display(), e))?;
    let mut af = File::open(audio_path)
        .map_err(|e| format!("Failed opening audio part {}: {}", audio_path.display(), e))?;

    let (mvhd_timescale, v_track) = load_track_source(&mut vf, b"vide")?;
    let (_, a_track) = load_track_source(&mut af, b"soun")?;

    let v_chunks = group_into_chunks(0, &v_track);
    let a_chunks = group_into_chunks(1, &a_track);

    let mut all_chunks = Vec::with_capacity(v_chunks.len() + a_chunks.len());
    all_chunks.extend(v_chunks);
    all_chunks.extend(a_chunks);

    // Order strictly by chunk start time, placing audio right before video at ties
    all_chunks.sort_by(|x, y| match x.start_ns.cmp(&y.start_ns) {
        Ordering::Equal => y.track_index.cmp(&x.track_index),
        ord => ord,
    });

    let mut rel_chunk_offsets: [Vec<u64>; 2] = [Vec::new(), Vec::new()];
    let mut samples_per_chunk: [Vec<u32>; 2] = [Vec::new(), Vec::new()];
    let mut mdat_payload_len = 0u64;

    for chunk in &all_chunks {
        rel_chunk_offsets[chunk.track_index].push(mdat_payload_len);
        samples_per_chunk[chunk.track_index].push(chunk.samples.len() as u32);
        for s in &chunk.samples {
            mdat_payload_len += s.size as u64;
        }
    }

    let v_dur_mvhd = (v_track.total_duration as u128 * mvhd_timescale as u128
        / v_track.timescale as u128) as u64;
    let a_dur_mvhd = (a_track.total_duration as u128 * mvhd_timescale as u128
        / a_track.timescale as u128) as u64;
    let movie_dur_mvhd = v_dur_mvhd.max(a_dur_mvhd);

    let ftyp = make_box(b"ftyp", b"isom\x00\x00\x02\x00isomiso2mp41");
    let use_co64 = (mdat_payload_len + 16 * 1024 * 1024) > u32::MAX as u64;

    let build_moov = |base_mdat_data_offset: u64| -> Vec<u8> {
        let v_offs: Vec<u64> = rel_chunk_offsets[0]
            .iter()
            .map(|r| base_mdat_data_offset + *r)
            .collect();
        let a_offs: Vec<u64> = rel_chunk_offsets[1]
            .iter()
            .map(|r| base_mdat_data_offset + *r)
            .collect();

        let mut moov_children = Vec::new();
        moov_children.extend_from_slice(&build_mvhd(mvhd_timescale, movie_dur_mvhd));
        moov_children.extend_from_slice(&build_trak_box(
            &v_track,
            1,
            mvhd_timescale,
            true,
            &v_offs,
            &samples_per_chunk[0],
            use_co64,
        ));
        moov_children.extend_from_slice(&build_trak_box(
            &a_track,
            2,
            mvhd_timescale,
            false,
            &a_offs,
            &samples_per_chunk[1],
            use_co64,
        ));
        make_box(b"moov", &moov_children)
    };

    let moov_pass1 = build_moov(0);
    let mdat_total_len = 8u64 + mdat_payload_len;
    let mdat_hdr_len = if mdat_total_len > u32::MAX as u64 { 16u64 } else { 8u64 };
    let mdat_data_start = ftyp.len() as u64 + moov_pass1.len() as u64 + mdat_hdr_len;
    let moov_final = build_moov(mdat_data_start);
    if moov_final.len() != moov_pass1.len() {
        return Err("MP4 moov offset pass mismatch".into());
    }

    let tmp_out = output_path.with_extension("vortexmux.mp4");
    {
        let out_file = File::create(&tmp_out)
            .map_err(|e| format!("Failed creating {}: {}", tmp_out.display(), e))?;
        let mut writer = BufWriter::with_capacity(1024 * 1024, out_file);
        writer.write_all(&ftyp).map_err(|e| e.to_string())?;
        writer.write_all(&moov_final).map_err(|e| e.to_string())?;

        if mdat_hdr_len == 16 {
            writer.write_all(&1u32.to_be_bytes()).map_err(|e| e.to_string())?;
            writer.write_all(b"mdat").map_err(|e| e.to_string())?;
            writer
                .write_all(&(16u64 + mdat_payload_len).to_be_bytes())
                .map_err(|e| e.to_string())?;
        } else {
            writer
                .write_all(&((8u64 + mdat_payload_len) as u32).to_be_bytes())
                .map_err(|e| e.to_string())?;
            writer.write_all(b"mdat").map_err(|e| e.to_string())?;
        }

        let mut v_reader = BufReader::with_capacity(512 * 1024, vf);
        let mut a_reader = BufReader::with_capacity(256 * 1024, af);
        let mut v_pos = u64::MAX;
        let mut a_pos = u64::MAX;
        let mut scratch = vec![0u8; 256 * 1024];

        for chunk in &all_chunks {
            let (reader, cur_pos) = if chunk.track_index == 0 {
                (&mut v_reader, &mut v_pos)
            } else {
                (&mut a_reader, &mut a_pos)
            };
            for s in &chunk.samples {
                let sz = s.size as usize;
                if scratch.len() < sz {
                    scratch.resize(sz, 0);
                }
                if *cur_pos != s.file_offset {
                    reader
                        .seek(SeekFrom::Start(s.file_offset))
                        .map_err(|e| format!("Sample seek failed: {}", e))?;
                }
                reader
                    .read_exact(&mut scratch[..sz])
                    .map_err(|e| format!("Sample read failed: {}", e))?;
                writer
                    .write_all(&scratch[..sz])
                    .map_err(|e| format!("Mux write failed: {}", e))?;
                *cur_pos = s.file_offset + sz as u64;
            }
        }
        writer.flush().map_err(|e| e.to_string())?;
    }

    fs::rename(&tmp_out, output_path)
        .map_err(|e| format!("Rename muxed output failed: {}", e))?;
    Ok(())
}

/// Muxes downloaded video + audio into a single playable media file.
/// Uses Vortex's built-in pure-Rust progressive MP4 muxer first, falling back to `ffmpeg` if needed.
pub fn mux_video_and_audio(video_path: &Path, audio_path: &Path, output_path: &Path) -> Result<(), String> {
    if mux_fmp4_files(video_path, audio_path, output_path).is_ok() {
        return Ok(());
    }

    let tmp_out = output_path.with_extension("vortexff.mp4");
    for bin in ["ffmpeg", "/opt/homebrew/bin/ffmpeg", "/usr/local/bin/ffmpeg"] {
        if let Ok(out) = Command::new(bin)
            .args([
                "-y",
                "-i",
                &video_path.to_string_lossy(),
                "-i",
                &audio_path.to_string_lossy(),
                "-c",
                "copy",
                "-movflags",
                "+faststart",
            ])
            .arg(&tmp_out)
            .output()
        {
            if out.status.success() && tmp_out.exists() {
                fs::rename(&tmp_out, output_path)
                    .map_err(|e| format!("Rename muxed output failed: {}", e))?;
                return Ok(());
            }
        }
    }
    let _ = fs::remove_file(&tmp_out);
    Err("Pure-Rust MP4 muxer requires MP4/M4A tracks, or install ffmpeg".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_full_box(typ: &[u8; 4], ver: u8, flags: [u8; 3], body: &[u8]) -> Vec<u8> {
        let mut p = Vec::with_capacity(4 + body.len());
        p.push(ver);
        p.extend_from_slice(&flags);
        p.extend_from_slice(body);
        make_box(typ, &p)
    }

    fn build_synthetic_fmp4(
        timescale: u32,
        handler: &[u8; 4],
        sample_specs: &[(u64, &[&[u8]], u32, i32)], // (tfdt, sample_bytes_list, dur, cto)
    ) -> Vec<u8> {
        let ftyp = make_box(b"ftyp", b"iso5\x00\x00\x02\x00iso5mp41");

        let mut mvhd_body = vec![0u8; 96];
        write_be32(&mut mvhd_body[8..12], 1000);
        write_be32(&mut mvhd_body[92..96], 2);
        let mvhd = test_full_box(b"mvhd", 0, [0, 0, 0], &mvhd_body);

        let mut trex_body = vec![0u8; 20];
        write_be32(&mut trex_body[0..4], 1);
        let trex = test_full_box(b"trex", 0, [0, 0, 0], &trex_body);
        let mvex = make_box(b"mvex", &trex);

        let mut tkhd_body = vec![0u8; 80];
        write_be32(&mut tkhd_body[8..12], 1);
        let tkhd = test_full_box(b"tkhd", 0, [0, 0, 3], &tkhd_body);

        let mut mdhd_body = vec![0u8; 20];
        write_be32(&mut mdhd_body[8..12], timescale);
        let mdhd = test_full_box(b"mdhd", 0, [0, 0, 0], &mdhd_body);

        let mut hdlr_body = vec![0u8; 24];
        hdlr_body[4..8].copy_from_slice(handler);
        let hdlr = test_full_box(b"hdlr", 0, [0, 0, 0], &hdlr_body);

        let stsd = test_full_box(
            b"stsd",
            0,
            [0, 0, 0],
            &[1u32.to_be_bytes().as_slice(), &make_box(b"avc1", &[0u8; 16])].concat(),
        );
        let stbl = make_box(b"stbl", &stsd);
        let minf = make_box(b"minf", &stbl);
        let mdia = make_box(b"mdia", &[mdhd, hdlr, minf].concat());
        let trak = make_box(b"trak", &[tkhd, mdia].concat());
        let moov = make_box(b"moov", &[mvhd, mvex, trak].concat());

        let mut file_bytes = Vec::new();
        file_bytes.extend_from_slice(&ftyp);
        file_bytes.extend_from_slice(&moov);

        for (seq, &(dt, samples_list, dur, cto)) in sample_specs.iter().enumerate() {
            let mfhd = test_full_box(b"mfhd", 0, [0, 0, 0], &((seq + 1) as u32).to_be_bytes());
            let mut tfhd_body = vec![0u8; 8];
            write_be32(&mut tfhd_body[0..4], 1);
            let tfhd = test_full_box(b"tfhd", 0, [0x02, 0x00, 0x00], &tfhd_body);
            let tfdt = test_full_box(b"tfdt", 0, [0, 0, 0], &(dt as u32).to_be_bytes());

            // trun with flags 0x0f01: data-offset(1) + dur(0x100) + size(0x200) + flags(0x400) + cto(0x800)
            let mut trun_pay = Vec::new();
            trun_pay.extend_from_slice(&(samples_list.len() as u32).to_be_bytes());
            trun_pay.extend_from_slice(&0i32.to_be_bytes()); // data_offset patched below
            let mut mdat_pay = Vec::new();
            for (idx, sb) in samples_list.iter().enumerate() {
                trun_pay.extend_from_slice(&dur.to_be_bytes());
                trun_pay.extend_from_slice(&(sb.len() as u32).to_be_bytes());
                let sf: u32 = if idx == 0 { 0 } else { 0x0001_0000 };
                trun_pay.extend_from_slice(&sf.to_be_bytes());
                trun_pay.extend_from_slice(&cto.to_be_bytes());
                mdat_pay.extend_from_slice(sb);
            }
            let trun_dummy = test_full_box(b"trun", 0, [0x00, 0x0f, 0x01], &trun_pay);
            let moof_len = 8 + mfhd.len() + 8 + tfhd.len() + tfdt.len() + trun_dummy.len();
            write_be32(&mut trun_pay[4..8], (moof_len + 8) as u32);

            let trun = test_full_box(b"trun", 0, [0x00, 0x0f, 0x01], &trun_pay);
            let traf = make_box(b"traf", &[tfhd, tfdt, trun].concat());
            let moof = make_box(b"moof", &[mfhd, traf].concat());
            let mdat = make_box(b"mdat", &mdat_pay);
            file_bytes.extend_from_slice(&moof);
            file_bytes.extend_from_slice(&mdat);
        }

        file_bytes
    }

    #[test]
    fn test_pure_rust_progressive_mp4_mux_interleaves_and_reconstructs_samples() {
        let dir = std::env::temp_dir().join("vortex_prog_mp4_mux_test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let v_path = dir.join("video.mp4");
        let a_path = dir.join("audio.m4a");
        let out_path = dir.join("merged.mp4");

        // Video: 2 fragments (3s each -> ~6s), Audio: 1 huge 6s fragment -> muxer chunks every 0.5s
        let v_f1 = [b"V1A".as_slice(), b"V1B".as_slice(), b"V1C".as_slice()];
        let v_f2 = [b"V2A".as_slice(), b"V2B".as_slice(), b"V2C".as_slice()];
        let a_f1 = [
            b"A1".as_slice(),
            b"A2".as_slice(),
            b"A3".as_slice(),
            b"A4".as_slice(),
            b"A5".as_slice(),
            b"A6".as_slice(),
        ];

        fs::write(
            &v_path,
            build_synthetic_fmp4(
                1000,
                b"vide",
                &[(0, &v_f1, 1000, 500), (3000, &v_f2, 1000, 500)],
            ),
        )
        .unwrap();
        fs::write(
            &a_path,
            build_synthetic_fmp4(44100, b"soun", &[(0, &a_f1, 44100, 0)]),
        )
        .unwrap();

        mux_fmp4_files(&v_path, &a_path, &out_path).unwrap();

        let mut f = File::open(&out_path).unwrap();
        let top = scan_file_top_boxes(&mut f).unwrap();
        assert_eq!(top.len(), 3);
        assert_eq!(&top[0].typ, b"ftyp");
        assert_eq!(&top[1].typ, b"moov");
        assert_eq!(&top[2].typ, b"mdat");

        let out = fs::read(&out_path).unwrap();
        let moov = &out[top[1].offset as usize..(top[1].offset + top[1].size) as usize];
        let mut trak_ids = Vec::new();
        let mut chunk_counts = Vec::new();
        for (t, c, hdr) in split_mem_boxes(&moov[8..]) {
            assert_ne!(&t, b"mvex", "output must be non-fragmented MP4");
            if &t == b"mvhd" {
                assert_eq!(read_be32(&c[hdr + 96..hdr + 100]), 3);
            } else if &t == b"trak" {
                for (tt, tc, thdr) in split_mem_boxes(&c[hdr..]) {
                    if &tt == b"tkhd" {
                        trak_ids.push(read_be32(&tc[thdr + 12..thdr + 16]));
                    } else if &tt == b"mdia" {
                        for (mt, mc, mhdr) in split_mem_boxes(&tc[thdr..]) {
                            if &mt == b"minf" {
                                for (nt, nc, nhdr) in split_mem_boxes(&mc[mhdr..]) {
                                    if &nt == b"stbl" {
                                        for (st, sc, shdr) in split_mem_boxes(&nc[nhdr..]) {
                                            if &st == b"stco" {
                                                chunk_counts.push(read_be32(&sc[shdr + 4..shdr + 8]));
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(trak_ids, vec![1, 2]);
        // Both tracks should be split into 6 tightly interleaved 1s chunks instead of 1 monolithic fragment
        assert_eq!(chunk_counts, vec![6, 6]);

        // Verify source tracks reload cleanly from the remuxed progressive MP4 itself!
        let mut mf = File::open(&out_path).unwrap();
        let (_, reloaded_v) = load_track_source(&mut mf, b"vide").unwrap();
        let (_, reloaded_a) = load_track_source(&mut mf, b"soun").unwrap();
        assert_eq!(reloaded_v.samples.len(), 6);
        assert_eq!(reloaded_a.samples.len(), 6);
        assert_eq!(
            &out[reloaded_a.samples[0].file_offset as usize
                ..reloaded_a.samples[0].file_offset as usize + reloaded_a.samples[0].size as usize],
            b"A1"
        );
        assert_eq!(
            &out[reloaded_v.samples[5].file_offset as usize
                ..reloaded_v.samples[5].file_offset as usize + reloaded_v.samples[5].size as usize],
            b"V2C"
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
