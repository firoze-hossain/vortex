//! vortex-core/src/media/mp4_mux.rs
//! Pure-Rust fragmented ISO-BMFF (fMP4) 2-track muxer (Video Track #1 + Audio Track #2).
//! Combines DASH split streams (e.g. YouTube 4K/1440p/1080p MP4 + M4A audio) into one
//! standard playable `.mp4` without requiring external binaries.

use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::process::Command;

#[derive(Clone, Debug)]
struct BoxSpan {
    typ: [u8; 4],
    offset: u64,
    size: u64,
}

#[derive(Clone, Debug)]
struct FragmentMeta {
    is_audio: bool,
    time_secs: f64,
    moof_offset: u64,
    moof_size: u64,
    mdat_offset: u64,
    mdat_size: u64,
}

fn read_be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

fn read_be64(b: &[u8]) -> u64 {
    u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
}

fn write_be32(b: &mut [u8], val: u32) {
    b[..4].copy_from_slice(&val.to_be_bytes());
}

fn make_box(typ: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + payload.len());
    let total = (8 + payload.len()) as u32;
    out.extend_from_slice(&total.to_be_bytes());
    out.extend_from_slice(typ);
    out.extend_from_slice(payload);
    out
}

fn scan_file_top_boxes(file: &mut File) -> Result<Vec<BoxSpan>, String> {
    let file_len = file.metadata().map_err(|e| format!("Stat error: {}", e))?.len();
    let mut pos = 0u64;
    let mut boxes = Vec::new();
    let mut hdr = [0u8; 16];

    while pos + 8 <= file_len {
        file.seek(SeekFrom::Start(pos)).map_err(|e| format!("Seek error: {}", e))?;
        file.read_exact(&mut hdr[..8]).map_err(|e| format!("Read header error: {}", e))?;
        let sz32 = read_be32(&hdr[0..4]) as u64;
        let typ = [hdr[4], hdr[5], hdr[6], hdr[7]];
        let (size, hdr_len) = if sz32 == 1 {
            if pos + 16 > file_len {
                break;
            }
            file.read_exact(&mut hdr[8..16]).map_err(|e| format!("Read 64-bit size error: {}", e))?;
            (read_be64(&hdr[8..16]), 16u64)
        } else if sz32 == 0 {
            (file_len - pos, 8u64)
        } else {
            (sz32, 8u64)
        };

        if size < hdr_len || pos + size > file_len {
            break;
        }
        boxes.push(BoxSpan {
            typ,
            offset: pos,
            size,
        });
        pos += size;
    }

    Ok(boxes)
}

fn read_box_bytes(file: &mut File, span: &BoxSpan) -> Result<Vec<u8>, String> {
    if span.size > 64 * 1024 * 1024 {
        return Err("Metadata box unexpectedly large".into());
    }
    let mut buf = vec![0u8; span.size as usize];
    file.seek(SeekFrom::Start(span.offset)).map_err(|e| format!("Seek error: {}", e))?;
    file.read_exact(&mut buf).map_err(|e| format!("Read box error: {}", e))?;
    Ok(buf)
}

fn split_mem_boxes(data: &[u8]) -> Vec<([u8; 4], &[u8], usize)> {
    let mut pos = 0usize;
    let mut out = Vec::new();
    while pos + 8 <= data.len() {
        let sz32 = read_be32(&data[pos..pos + 4]) as usize;
        let typ = [data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7]];
        let (size, hdr_len) = if sz32 == 1 && pos + 16 <= data.len() {
            (read_be64(&data[pos + 8..pos + 16]) as usize, 16usize)
        } else if sz32 == 0 {
            (data.len() - pos, 8usize)
        } else {
            (sz32, 8usize)
        };
        if size < hdr_len || pos + size > data.len() {
            break;
        }
        out.push((typ, &data[pos..pos + size], hdr_len));
        pos += size;
    }
    out
}

fn extract_timescale_from_trak(trak_box: &[u8], trak_hdr: usize) -> u32 {
    for (t, box_bytes, hdr) in split_mem_boxes(&trak_box[trak_hdr..]) {
        if &t == b"mdia" {
            for (mt, mbox, mhdr) in split_mem_boxes(&box_bytes[hdr..]) {
                if &mt == b"mdhd" && mbox.len() >= mhdr + 16 {
                    let ver = mbox[mhdr];
                    let off = if ver == 0 { mhdr + 12 } else { mhdr + 20 };
                    if mbox.len() >= off + 4 {
                        return read_be32(&mbox[off..off + 4]).max(1);
                    }
                }
            }
        }
    }
    1000
}

fn patch_tkhd_track_id(trak_box: &[u8], trak_hdr: usize, new_track_id: u32) -> Result<Vec<u8>, String> {
    let mut payload = Vec::with_capacity(trak_box.len());
    let mut patched = false;
    for (t, child, hdr) in split_mem_boxes(&trak_box[trak_hdr..]) {
        if &t == b"tkhd" && child.len() >= hdr + 24 {
            let mut copy = child.to_vec();
            let ver = copy[hdr];
            let id_off = if ver == 0 { hdr + 12 } else { hdr + 20 };
            if copy.len() >= id_off + 4 {
                write_be32(&mut copy[id_off..id_off + 4], new_track_id);
                patched = true;
            }
            payload.extend_from_slice(&copy);
        } else {
            payload.extend_from_slice(child);
        }
    }
    if !patched {
        return Err("Missing tkhd in audio trak".into());
    }
    Ok(make_box(b"trak", &payload))
}

fn merge_moov(v_moov: &[u8], a_moov: &[u8]) -> Result<(Vec<u8>, u32, u32), String> {
    let mut v_mvhd: Option<Vec<u8>> = None;
    let mut v_mvex_children: Vec<Vec<u8>> = Vec::new();
    let mut v_traks: Vec<Vec<u8>> = Vec::new();
    let mut v_timescale = 1000u32;

    for (t, child, hdr) in split_mem_boxes(&v_moov[8..]) {
        match &t {
            b"mvhd" => {
                let mut m = child.to_vec();
                if m.len() >= hdr + 100 {
                    let ver = m[hdr];
                    let next_off = if ver == 0 { hdr + 96 } else { hdr + 108 };
                    if m.len() >= next_off + 4 {
                        write_be32(&mut m[next_off..next_off + 4], 3);
                    }
                }
                v_mvhd = Some(m);
            }
            b"mvex" => {
                for (_, mc, _) in split_mem_boxes(&child[hdr..]) {
                    v_mvex_children.push(mc.to_vec());
                }
            }
            b"trak" => {
                v_timescale = extract_timescale_from_trak(child, hdr);
                v_traks.push(child.to_vec());
            }
            _ => {}
        }
    }

    let mut a_trex_patched: Option<Vec<u8>> = None;
    let mut a_traks_patched: Vec<Vec<u8>> = Vec::new();
    let mut a_timescale = 44100u32;

    for (t, child, hdr) in split_mem_boxes(&a_moov[8..]) {
        match &t {
            b"mvex" => {
                for (mt, mc, mhdr) in split_mem_boxes(&child[hdr..]) {
                    if &mt == b"trex" && mc.len() >= mhdr + 8 {
                        let mut c = mc.to_vec();
                        write_be32(&mut c[mhdr + 4..mhdr + 8], 2);
                        a_trex_patched = Some(c);
                    }
                }
            }
            b"trak" => {
                a_timescale = extract_timescale_from_trak(child, hdr);
                a_traks_patched.push(patch_tkhd_track_id(child, hdr, 2)?);
            }
            _ => {}
        }
    }

    let mvhd = v_mvhd.ok_or_else(|| "Missing video mvhd box".to_string())?;
    let a_trex = a_trex_patched.ok_or_else(|| "Missing audio mvex/trex box (not fMP4)".to_string())?;
    if v_traks.is_empty() || a_traks_patched.is_empty() {
        return Err("Missing video or audio trak box".into());
    }

    let mut mvex_payload = Vec::new();
    for mc in v_mvex_children {
        mvex_payload.extend_from_slice(&mc);
    }
    mvex_payload.extend_from_slice(&a_trex);
    let merged_mvex = make_box(b"mvex", &mvex_payload);

    let mut moov_payload = Vec::new();
    moov_payload.extend_from_slice(&mvhd);
    moov_payload.extend_from_slice(&merged_mvex);
    for vt in v_traks {
        moov_payload.extend_from_slice(&vt);
    }
    for at in a_traks_patched {
        moov_payload.extend_from_slice(&at);
    }

    Ok((make_box(b"moov", &moov_payload), v_timescale, a_timescale))
}

fn read_moof_tfdt_and_patch(
    moof_bytes: &mut [u8],
    new_seq: u32,
    new_track_id: Option<u32>,
) -> u64 {
    let mut tfdt_val = 0u64;
    let mut pos = 8usize;
    while pos + 8 <= moof_bytes.len() {
        let sz = read_be32(&moof_bytes[pos..pos + 4]) as usize;
        let typ = [
            moof_bytes[pos + 4],
            moof_bytes[pos + 5],
            moof_bytes[pos + 6],
            moof_bytes[pos + 7],
        ];
        if sz < 8 || pos + sz > moof_bytes.len() {
            break;
        }
        if &typ == b"mfhd" && sz >= 16 {
            write_be32(&mut moof_bytes[pos + 12..pos + 16], new_seq);
        } else if &typ == b"traf" {
            let mut tpos = pos + 8;
            let tend = pos + sz;
            while tpos + 8 <= tend {
                let tsz = read_be32(&moof_bytes[tpos..tpos + 4]) as usize;
                let ttyp = [
                    moof_bytes[tpos + 4],
                    moof_bytes[tpos + 5],
                    moof_bytes[tpos + 6],
                    moof_bytes[tpos + 7],
                ];
                if tsz < 8 || tpos + tsz > tend {
                    break;
                }
                if &ttyp == b"tfhd" && tsz >= 16 {
                    if let Some(tid) = new_track_id {
                        write_be32(&mut moof_bytes[tpos + 12..tpos + 16], tid);
                    }
                } else if &ttyp == b"tfdt" && tsz >= 16 {
                    let ver = moof_bytes[tpos + 8];
                    tfdt_val = if ver == 0 {
                        read_be32(&moof_bytes[tpos + 12..tpos + 16]) as u64
                    } else if tsz >= 20 {
                        read_be64(&moof_bytes[tpos + 12..tpos + 20])
                    } else {
                        0
                    };
                }
                tpos += tsz;
            }
        }
        pos += sz;
    }
    tfdt_val
}

fn collect_fragments(
    file: &mut File,
    spans: &[BoxSpan],
    is_audio: bool,
    timescale: u32,
) -> Result<Vec<FragmentMeta>, String> {
    let mut frags = Vec::new();
    let mut i = 0;
    while i + 1 < spans.len() {
        if &spans[i].typ == b"moof" && &spans[i + 1].typ == b"mdat" {
            let mut moof = read_box_bytes(file, &spans[i])?;
            let tfdt = read_moof_tfdt_and_patch(&mut moof, 1, None);
            frags.push(FragmentMeta {
                is_audio,
                time_secs: tfdt as f64 / (timescale.max(1) as f64),
                moof_offset: spans[i].offset,
                moof_size: spans[i].size,
                mdat_offset: spans[i + 1].offset,
                mdat_size: spans[i + 1].size,
            });
            i += 2;
        } else {
            i += 1;
        }
    }
    Ok(frags)
}

fn copy_exact_range(src: &mut File, offset: u64, size: u64, dst: &mut BufWriter<File>) -> Result<(), String> {
    src.seek(SeekFrom::Start(offset)).map_err(|e| format!("Seek error: {}", e))?;
    let mut rem = size;
    let mut buf = [0u8; 64 * 1024];
    while rem > 0 {
        let take = (rem as usize).min(buf.len());
        let n = src.read(&mut buf[..take]).map_err(|e| format!("Read error: {}", e))?;
        if n == 0 {
            return Err("Unexpected EOF while copying fragment data".into());
        }
        dst.write_all(&buf[..n]).map_err(|e| format!("Write error: {}", e))?;
        rem -= n as u64;
    }
    Ok(())
}

/// Pure-Rust fragmented MP4 (fMP4) 2-track video + audio muxer.
pub fn mux_fmp4_files(video_path: &Path, audio_path: &Path, output_path: &Path) -> Result<(), String> {
    let mut v_file = File::open(video_path).map_err(|e| format!("Cannot open video: {}", e))?;
    let mut a_file = File::open(audio_path).map_err(|e| format!("Cannot open audio: {}", e))?;

    let v_spans = scan_file_top_boxes(&mut v_file)?;
    let a_spans = scan_file_top_boxes(&mut a_file)?;

    let ftyp_span = v_spans
        .iter()
        .find(|b| &b.typ == b"ftyp")
        .ok_or_else(|| "Video missing ftyp box".to_string())?;
    let v_moov_span = v_spans
        .iter()
        .find(|b| &b.typ == b"moov")
        .ok_or_else(|| "Video missing moov box".to_string())?;
    let a_moov_span = a_spans
        .iter()
        .find(|b| &b.typ == b"moov")
        .ok_or_else(|| "Audio missing moov box".to_string())?;

    let ftyp_bytes = read_box_bytes(&mut v_file, ftyp_span)?;
    let v_moov_bytes = read_box_bytes(&mut v_file, v_moov_span)?;
    let a_moov_bytes = read_box_bytes(&mut a_file, a_moov_span)?;

    let (merged_moov, v_ts, a_ts) = merge_moov(&v_moov_bytes, &a_moov_bytes)?;

    let mut all_frags = collect_fragments(&mut v_file, &v_spans, false, v_ts)?;
    let a_frags = collect_fragments(&mut a_file, &a_spans, true, a_ts)?;
    if all_frags.is_empty() || a_frags.is_empty() {
        return Err("Missing fMP4 moof/mdat fragments (not fragmented MP4)".into());
    }
    all_frags.extend(a_frags);

    // Interleave chronologically so audio & video fragments align smoothly in players & scrubbing
    all_frags.sort_by(|a, b| {
        a.time_secs
            .partial_cmp(&b.time_secs)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.is_audio.cmp(&b.is_audio))
    });

    let tmp_out = output_path.with_extension("vortexmux.tmp");
    let out_file = File::create(&tmp_out).map_err(|e| format!("Cannot create output MP4: {}", e))?;
    let mut writer = BufWriter::new(out_file);

    writer.write_all(&ftyp_bytes).map_err(|e| format!("Write ftyp error: {}", e))?;
    writer.write_all(&merged_moov).map_err(|e| format!("Write moov error: {}", e))?;

    let mut v_reader = BufReader::new(v_file);
    let mut a_reader = BufReader::new(a_file);

    for (idx, frag) in all_frags.iter().enumerate() {
        let src = if frag.is_audio { a_reader.get_mut() } else { v_reader.get_mut() };
        let moof_span = BoxSpan {
            typ: *b"moof",
            offset: frag.moof_offset,
            size: frag.moof_size,
        };
        let mut moof_buf = read_box_bytes(src, &moof_span)?;
        read_moof_tfdt_and_patch(
            &mut moof_buf,
            (idx + 1) as u32,
            if frag.is_audio { Some(2) } else { None },
        );
        writer.write_all(&moof_buf).map_err(|e| format!("Write moof error: {}", e))?;
        copy_exact_range(src, frag.mdat_offset, frag.mdat_size, &mut writer)?;
    }

    writer.flush().map_err(|e| format!("Flush error: {}", e))?;
    drop(writer);

    fs::rename(&tmp_out, output_path).map_err(|e| {
        let _ = fs::remove_file(&tmp_out);
        format!("Finalize muxed MP4 failed: {}", e)
    })?;
    Ok(())
}

/// Muxes downloaded video + audio into a single playable media file.
/// Uses Vortex's built-in pure-Rust fMP4 muxer first, falling back to `ffmpeg` if needed.
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
    Err("Pure-Rust fMP4 muxer requires fragmented MP4 + M4A, or install ffmpeg".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_full_box(typ: &[u8; 4], ver: u8, flags: [u8; 3], body: &[u8]) -> Vec<u8> {
        let mut p = Vec::with_capacity(4 + body.len());
        p.push(ver);
        p.extend_from_slice(&flags);
        p.extend_from_slice(body);
        make_box(typ, &p)
    }

    fn build_synthetic_fmp4(timescale: u32, tfdt_times: &[u64]) -> Vec<u8> {
        let ftyp = make_box(b"ftyp", b"iso5\x00\x00\x02\x00iso5mp41");

        // mvhd v0: 4 + 96 = 100 bytes payload (total 108 bytes)
        let mut mvhd_body = vec![0u8; 96];
        write_be32(&mut mvhd_body[92..96], 2); // next_track_ID = 2
        let mvhd = make_full_box(b"mvhd", 0, [0, 0, 0], &mvhd_body);

        // trex: 4 + 20 bytes
        let mut trex_body = vec![0u8; 20];
        write_be32(&mut trex_body[0..4], 1);
        let trex = make_full_box(b"trex", 0, [0, 0, 0], &trex_body);
        let mvex = make_box(b"mvex", &trex);

        // tkhd v0: 4 + 80 bytes
        let mut tkhd_body = vec![0u8; 80];
        write_be32(&mut tkhd_body[8..12], 1);
        let tkhd = make_full_box(b"tkhd", 0, [0, 0, 3], &tkhd_body);

        // mdhd v0: 4 + 20 bytes
        let mut mdhd_body = vec![0u8; 20];
        write_be32(&mut mdhd_body[8..12], timescale);
        let mdhd = make_full_box(b"mdhd", 0, [0, 0, 0], &mdhd_body);
        let mdia = make_box(b"mdia", &mdhd);
        let trak = make_box(b"trak", &[tkhd, mdia].concat());

        let moov = make_box(b"moov", &[mvhd, mvex, trak].concat());

        let mut file_bytes = Vec::new();
        file_bytes.extend_from_slice(&ftyp);
        file_bytes.extend_from_slice(&moov);

        for (seq, &dt) in tfdt_times.iter().enumerate() {
            let mut mfhd_body = [0u8; 4];
            write_be32(&mut mfhd_body, (seq + 1) as u32);
            let mfhd = make_full_box(b"mfhd", 0, [0, 0, 0], &mfhd_body);

            let mut tfhd_body = vec![0u8; 8];
            write_be32(&mut tfhd_body[0..4], 1); // track_id = 1
            let tfhd = make_full_box(b"tfhd", 0, [0x02, 0x00, 0x00], &tfhd_body);

            let mut tfdt_body = [0u8; 4];
            write_be32(&mut tfdt_body, dt as u32);
            let tfdt = make_full_box(b"tfdt", 0, [0, 0, 0], &tfdt_body);

            let traf = make_box(b"traf", &[tfhd, tfdt].concat());
            let moof = make_box(b"moof", &[mfhd, traf].concat());
            let mdat = make_box(b"mdat", &[0xAAu8; 32]);
            file_bytes.extend_from_slice(&moof);
            file_bytes.extend_from_slice(&mdat);
        }

        file_bytes
    }

    #[test]
    fn test_pure_rust_fmp4_mux_interleaves_tracks_and_patches_ids() {
        let dir = std::env::temp_dir().join("vortex_fmp4_mux_test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let v_path = dir.join("video.mp4");
        let a_path = dir.join("audio.m4a");
        let out_path = dir.join("merged.mp4");

        fs::write(&v_path, build_synthetic_fmp4(1000, &[0, 2000])).unwrap();
        fs::write(&a_path, build_synthetic_fmp4(44100, &[0, 44100, 88200])).unwrap();

        mux_fmp4_files(&v_path, &a_path, &out_path).unwrap();
        let out = fs::read(&out_path).unwrap();

        let mut f = File::open(&out_path).unwrap();
        let top = scan_file_top_boxes(&mut f).unwrap();
        assert_eq!(&top[0].typ, b"ftyp");
        assert_eq!(&top[1].typ, b"moov");

        // Verify moov contains next_track_id=3, two traks (ids 1 & 2), and two trex (ids 1 & 2)
        let moov = &out[top[1].offset as usize..(top[1].offset + top[1].size) as usize];
        let mut trak_ids = Vec::new();
        let mut trex_ids = Vec::new();
        for (t, c, hdr) in split_mem_boxes(&moov[8..]) {
            if &t == b"mvhd" {
                assert_eq!(read_be32(&c[hdr + 96..hdr + 100]), 3);
            } else if &t == b"mvex" {
                for (mt, mc, mhdr) in split_mem_boxes(&c[hdr..]) {
                    if &mt == b"trex" {
                        trex_ids.push(read_be32(&mc[mhdr + 4..mhdr + 8]));
                    }
                }
            } else if &t == b"trak" {
                for (tt, tc, thdr) in split_mem_boxes(&c[hdr..]) {
                    if &tt == b"tkhd" {
                        trak_ids.push(read_be32(&tc[thdr + 12..thdr + 16]));
                    }
                }
            }
        }
        assert_eq!(trak_ids, vec![1, 2]);
        assert_eq!(trex_ids, vec![1, 2]);

        // Verify 5 interleaved moof+mdat fragments with sequential sequence_number and track IDs 1 & 2
        let moofs: Vec<_> = top.iter().filter(|b| &b.typ == b"moof").collect();
        assert_eq!(moofs.len(), 5);
        let mut seen_tids = Vec::new();
        for (i, m) in moofs.iter().enumerate() {
            let mut mbytes = out[m.offset as usize..(m.offset + m.size) as usize].to_vec();
            for (ct, cc, chdr) in split_mem_boxes(&mbytes[8..]) {
                if &ct == b"mfhd" {
                    assert_eq!(read_be32(&cc[chdr + 4..chdr + 8]), (i + 1) as u32);
                } else if &ct == b"traf" {
                    for (tt, tc, thdr) in split_mem_boxes(&cc[chdr..]) {
                        if &tt == b"tfhd" {
                            seen_tids.push(read_be32(&tc[thdr + 4..thdr + 8]));
                        }
                    }
                }
            }
            let _ = &mut mbytes;
        }
        assert!(seen_tids.contains(&1));
        assert!(seen_tids.contains(&2));
        let _ = fs::remove_dir_all(&dir);
    }
}
