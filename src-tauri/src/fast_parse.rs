//! High-throughput targeted SFNT table reader & streaming file hasher.
//!
//! Benchmarks on Windows (`C:\Windows\Fonts`, 131 fonts, 131.5 MB) showed reading
//! only the SFNT table directory + metadata tables is ~22x faster than `std::fs::read`
//! because glyph outlines (`glyf`, `CFF `, `CFF2`, `COLR`, `CBDT`, `sbix`, `SVG `)
//! account for 80–95% of file bytes and are unnecessary for metadata, axis, and
//! classification inspection.

use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Tables required by `ttf_parser::Face::parse` + our metadata/layout extractor.
/// Excludes `glyf`, `loca`, `CFF `, `CFF2`, `COLR`, `CPAL`, `CBDT`, `CBLC`,
/// `sbix`, `SVG `, `cmap`, `kern`, `EBDT`, `EBLC`.
const METADATA_TAGS: &[[u8; 4]] = &[
    *b"head",
    *b"hhea",
    *b"maxp",
    *b"OS/2",
    *b"name",
    *b"post",
    *b"fvar",
    *b"STAT",
    *b"GSUB",
    *b"GPOS",
];

/// Open a font file for read without blocking concurrent Windows readers, writers, or deletes.
pub fn open_shared_read(path: &Path) -> std::io::Result<File> {
    let mut opts = OpenOptions::new();
    opts.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_SHARE_READ (1) | FILE_SHARE_WRITE (2) | FILE_SHARE_DELETE (4)
        opts.share_mode(0x07);
    }
    opts.open(path)
}

/// Stream-hash a file with SHA-256 in 64 KB chunks without holding huge font binaries in RAM.
pub fn sha256_file_streaming(path: &Path) -> std::io::Result<String> {
    let mut file = open_shared_read(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let digest = hasher.finalize();
    Ok(digest.iter().map(|b| format!("{b:02x}")).collect())
}

fn is_single_sfnt(tag: &[u8; 4]) -> bool {
    matches!(tag, [0, 1, 0, 0] | b"OTTO" | b"true" | b"typ1")
}

/// Read ONLY the SFNT header and metadata tables needed for `layout_from_face` and
/// `face_installed_names`, repacking them into a compact valid SFNT byte buffer for
/// `ttf_parser::Face::parse`.
///
/// Returns `Ok(None)` for `.ttc`/`.otc` collections or non-SFNT files so callers can
/// fall back to full parsing cleanly.
pub fn read_metadata_sfnt_bytes(path: &Path) -> std::io::Result<Option<Vec<u8>>> {
    let mut file = open_shared_read(path)?;
    let file_len = file.metadata()?.len();
    if file_len < 12 {
        return Ok(None);
    }
    // Small files (< 48 KB) are cheaper to read in a single syscall than multiple seeks.
    if file_len <= 48 * 1024 {
        let mut all = Vec::with_capacity(file_len as usize);
        file.read_to_end(&mut all)?;
        if all.len() >= 4 && is_single_sfnt(&[all[0], all[1], all[2], all[3]]) {
            return Ok(Some(all));
        }
        return Ok(None);
    }

    // Read SFNT offset table (12 bytes) + up to 255 table records (4080 bytes) in one read.
    let head_len = file_len.min(4096) as usize;
    let mut header = vec![0u8; head_len];
    file.read_exact(&mut header)?;

    let sfnt_tag = [header[0], header[1], header[2], header[3]];
    if !is_single_sfnt(&sfnt_tag) {
        return Ok(None);
    }
    let num_tables = u16::from_be_bytes([header[4], header[5]]) as usize;
    if num_tables == 0 || num_tables > 256 {
        return Ok(None);
    }
    let dir_end = 12 + num_tables * 16;
    if dir_end > header.len() {
        if dir_end as u64 > file_len {
            return Ok(None);
        }
        header.resize(dir_end, 0);
        file.read_exact(&mut header[head_len..])?;
    }

    struct RawRec {
        tag: [u8; 4],
        checksum: u32,
        offset: u32,
        length: u32,
    }

    let mut wanted: Vec<RawRec> = Vec::with_capacity(METADATA_TAGS.len());
    for i in 0..num_tables {
        let off = 12 + i * 16;
        let tag = [header[off], header[off + 1], header[off + 2], header[off + 3]];
        if !METADATA_TAGS.iter().any(|t| t == &tag) {
            continue;
        }
        let checksum = u32::from_be_bytes([
            header[off + 4],
            header[off + 5],
            header[off + 6],
            header[off + 7],
        ]);
        let offset = u32::from_be_bytes([
            header[off + 8],
            header[off + 9],
            header[off + 10],
            header[off + 11],
        ]);
        let length = u32::from_be_bytes([
            header[off + 12],
            header[off + 13],
            header[off + 14],
            header[off + 15],
        ]);
        // Guard against corrupt offsets or huge GSUB/GPOS (> 4 MB) — fall back cleanly.
        if u64::from(offset) + u64::from(length) > file_len || length > 4 * 1024 * 1024 {
            return Ok(None);
        }
        wanted.push(RawRec {
            tag,
            checksum,
            offset,
            length,
        });
    }

    // `head` and `maxp` are mandatory for `ttf_parser::Face::parse`.
    let has_head = wanted.iter().any(|r| &r.tag == b"head");
    let has_maxp = wanted.iter().any(|r| &r.tag == b"maxp");
    if !has_head || !has_maxp {
        return Ok(None);
    }

    // Sort by file offset so disk seeks move forward sequentially.
    wanted.sort_by_key(|r| r.offset);

    let mut payloads: Vec<([u8; 4], u32, Vec<u8>)> = Vec::with_capacity(wanted.len());
    for rec in &wanted {
        let mut data = vec![0u8; rec.length as usize];
        let end = rec.offset as usize + rec.length as usize;
        if end <= header.len() {
            data.copy_from_slice(&header[rec.offset as usize..end]);
        } else {
            file.seek(SeekFrom::Start(u64::from(rec.offset)))?;
            file.read_exact(&mut data)?;
        }
        payloads.push((rec.tag, rec.checksum, data));
    }

    // SFNT specification requires table directory records sorted ascending by 4-byte tag.
    payloads.sort_by_key(|(tag, _, _)| *tag);

    let out_tables = payloads.len() as u16;
    let max_pow2 = 1u16 << (15 - out_tables.leading_zeros().min(15));
    let search_range = max_pow2 * 16;
    let entry_selector = max_pow2.trailing_zeros() as u16;
    let range_shift = out_tables.saturating_mul(16).saturating_sub(search_range);

    let pad4 = |n: usize| (n + 3) & !3;
    let dir_size = 12 + payloads.len() * 16;
    let total_size = dir_size + payloads.iter().map(|(_, _, d)| pad4(d.len())).sum::<usize>();
    let mut out = vec![0u8; total_size];

    out[0..4].copy_from_slice(&sfnt_tag);
    out[4..6].copy_from_slice(&out_tables.to_be_bytes());
    out[6..8].copy_from_slice(&search_range.to_be_bytes());
    out[8..10].copy_from_slice(&entry_selector.to_be_bytes());
    out[10..12].copy_from_slice(&range_shift.to_be_bytes());

    let mut cursor = dir_size;
    for (idx, (tag, checksum, data)) in payloads.into_iter().enumerate() {
        let r = 12 + idx * 16;
        out[r..r + 4].copy_from_slice(&tag);
        out[r + 4..r + 8].copy_from_slice(&checksum.to_be_bytes());
        out[r + 8..r + 12].copy_from_slice(&(cursor as u32).to_be_bytes());
        out[r + 12..r + 16].copy_from_slice(&(data.len() as u32).to_be_bytes());
        out[cursor..cursor + data.len()].copy_from_slice(&data);
        cursor += pad4(data.len());
    }

    Ok(Some(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use read_fonts::{FontRef, TableProvider};

    #[test]
    fn fast_metadata_slice_matches_full_parse_on_vf_fixtures() {
        let dir = std::env::temp_dir().join(format!("fm-fast-parse-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        for (name, raw) in [
            ("Figtree-wght.ttf", include_bytes!("../../tests/fixtures/Figtree-wght.ttf").as_slice()),
            ("Inter-opsz-wght.ttf", include_bytes!("../../tests/fixtures/Inter-opsz-wght.ttf").as_slice()),
        ] {
            let p = dir.join(name);
            std::fs::write(&p, raw).unwrap();
            let compact = read_metadata_sfnt_bytes(&p)
                .expect("fast read")
                .expect("single sfnt");
            assert!(
                compact.len() < raw.len(),
                "{name}: compact {} should be strictly smaller than full {}",
                compact.len(),
                raw.len()
            );

            let full_face = FontRef::new(raw).expect("full face");
            let fast_face = FontRef::new(&compact).expect("fast face");
            assert_eq!(
                full_face.maxp().unwrap().num_glyphs(),
                fast_face.maxp().unwrap().num_glyphs()
            );
            assert_eq!(full_face.fvar().is_ok(), fast_face.fvar().is_ok());
            assert_eq!(
                full_face.os2().unwrap().us_weight_class(),
                fast_face.os2().unwrap().us_weight_class()
            );
            assert_eq!(
                full_face.post().unwrap().is_fixed_pitch(),
                fast_face.post().unwrap().is_fixed_pitch()
            );

            let full_axes: Vec<String> = full_face
                .fvar()
                .unwrap()
                .axes()
                .unwrap()
                .iter()
                .map(|a| a.axis_tag().to_string())
                .collect();
            let fast_axes: Vec<String> = fast_face
                .fvar()
                .unwrap()
                .axes()
                .unwrap()
                .iter()
                .map(|a| a.axis_tag().to_string())
                .collect();
            assert_eq!(full_axes, fast_axes);

            // Verify streaming SHA-256 matches in-memory SHA-256
            let mut h = Sha256::new();
            h.update(raw);
            let expected_hash: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
            assert_eq!(sha256_file_streaming(&p).unwrap(), expected_hash);
        }

        let _ = std::fs::remove_dir_all(&dir);
    }
}
