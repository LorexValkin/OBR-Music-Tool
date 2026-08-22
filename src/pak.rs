//! Reader for Unreal Engine pak files (v10+, unencrypted index): index parsing
//! and file extraction. Shared between the application (sound preview) and
//! `tools/sfxindex` (verification), so this file depends on `std` and `anyhow`
//! only; block decompression goes through [`crate::iostore::Decompressor`].

use crate::iostore::Decompressor;
use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

const PAK_MAGIC: [u8; 4] = [0xe1, 0x12, 0x6f, 0x5a];

/// One file inside the pak.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PakEntry {
    /// Absolute offset of the entry (its on-disk header) in the pak.
    pub offset: u64,
    pub uncompressed_size: u64,
    /// Stored size (equals `uncompressed_size` when not compressed).
    pub size: u64,
    /// 0 = stored raw, otherwise `1 + index` into [`PakIndex::methods`].
    pub method: u8,
    pub encrypted: bool,
    /// Uncompressed bytes per block (the last block may be shorter).
    pub block_size: u64,
    /// `(start, end)` of each compressed block, relative to `offset`.
    pub blocks: Vec<(u64, u64)>,
    /// Size of the serialized entry header that precedes the data on disk.
    pub header_size: u64,
}

pub struct PakIndex {
    pub version: u32,
    pub mount_point: String,
    /// Compression method names; entry method `m >= 1` maps to `methods[m - 1]`.
    pub methods: Vec<String>,
    /// Files that passed the filter, keyed by their path relative to `WwiseAudio/`.
    pub entries: HashMap<String, PakEntry>,
    /// Total number of files in the pak.
    pub file_count: usize,
    pub index_hash: [u8; 20],
}

fn u32_at(b: &[u8], o: usize) -> Result<u32> {
    b.get(o..o + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .context("pak: truncated")
}

fn i64_at(b: &[u8], o: usize) -> Result<i64> {
    let mut a = [0u8; 8];
    a.copy_from_slice(b.get(o..o + 8).context("pak: truncated")?);
    Ok(i64::from_le_bytes(a))
}

fn fstring(b: &[u8], cursor: &mut usize) -> Result<String> {
    let len = u32_at(b, *cursor)? as i32;
    *cursor += 4;
    if len == 0 {
        return Ok(String::new());
    }
    if len > 0 {
        let n = len as usize;
        let s = b.get(*cursor..*cursor + n).context("pak: truncated string")?;
        *cursor += n;
        let end = s.iter().position(|&c| c == 0).unwrap_or(n);
        Ok(String::from_utf8_lossy(&s[..end]).into_owned())
    } else {
        let n = (-(len as i64)) as usize * 2;
        let s = b.get(*cursor..*cursor + n).context("pak: truncated wide string")?;
        *cursor += n;
        let units: Vec<u16> = s.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&u| u != 0).collect();
        Ok(String::from_utf16_lossy(&units))
    }
}

/// Decode one "encoded" pak entry (the compact v10+ layout used by the index).
pub fn decode_entry(enc: &[u8], offset: usize) -> Result<PakEntry> {
    let flags = u32_at(enc, offset)?;
    let mut c = offset + 4;
    let mut block_size = if flags & 0x3f == 0x3f {
        let v = u32_at(enc, c)? as u64;
        c += 4;
        v
    } else {
        ((flags & 0x3f) as u64) << 11
    };
    let method = ((flags >> 23) & 0x3f) as u8;
    let entry_offset = if flags & (1 << 31) != 0 {
        let v = u32_at(enc, c)? as u64;
        c += 4;
        v
    } else {
        let v = i64_at(enc, c)? as u64;
        c += 8;
        v
    };
    let uncompressed_size = if flags & (1 << 30) != 0 {
        let v = u32_at(enc, c)? as u64;
        c += 4;
        v
    } else {
        let v = i64_at(enc, c)? as u64;
        c += 8;
        v
    };
    let size = if method != 0 {
        if flags & (1 << 29) != 0 {
            let v = u32_at(enc, c)? as u64;
            c += 4;
            v
        } else {
            let v = i64_at(enc, c)? as u64;
            c += 8;
            v
        }
    } else {
        uncompressed_size
    };
    let encrypted = flags & (1 << 22) != 0;
    let block_count = ((flags >> 6) & 0xffff) as usize;
    // Serialized FPakEntry: offset, size, uncompressed size (8 each), method (4),
    // hash (20), [block count (4) + blocks (16 each)], encrypted flag (1), block size (4).
    let header_size = 53 + if method != 0 { 4 + 16 * block_count as u64 } else { 0 };
    let mut blocks = Vec::with_capacity(block_count);
    if method != 0 {
        if block_count == 1 {
            blocks.push((header_size, header_size + size));
            block_size = uncompressed_size;
        } else {
            let align = if encrypted { 16 } else { 1 };
            let mut start = header_size;
            for _ in 0..block_count {
                let len = u32_at(enc, c)? as u64;
                c += 4;
                blocks.push((start, start + len));
                start += (len + align - 1) / align * align;
            }
        }
    }
    Ok(PakEntry { offset: entry_offset, uncompressed_size, size, method, encrypted, block_size, blocks, header_size })
}

impl PakIndex {
    /// Read the index, keeping files under `WwiseAudio/` whose relative path
    /// satisfies `keep` (e.g. only `Media/*.wem`).
    pub fn read(path: &Path, keep: impl Fn(&str) -> bool) -> Result<PakIndex> {
        let mut f = File::open(path).with_context(|| format!("opening {}", path.display()))?;
        let total = f.metadata()?.len();
        let tail_len = total.min(4096) as usize;
        f.seek(SeekFrom::Start(total - tail_len as u64))?;
        let mut tail = vec![0u8; tail_len];
        f.read_exact(&mut tail)?;
        let m = tail
            .windows(4)
            .rposition(|w| w == PAK_MAGIC)
            .context("pak: footer magic not found")?;
        if m == 0 || tail[m - 1] != 0 {
            bail!("pak: index is encrypted");
        }
        let version = u32_at(&tail, m + 4)?;
        if version < 10 {
            bail!("pak: unsupported version {version}");
        }
        let index_offset = i64_at(&tail, m + 8)? as u64;
        let index_size = i64_at(&tail, m + 16)? as usize;
        let mut index_hash = [0u8; 20];
        index_hash.copy_from_slice(tail.get(m + 24..m + 44).context("pak: truncated footer")?);
        let mut methods = Vec::new();
        for i in 0..5 {
            let Some(name) = tail.get(m + 44 + i * 32..m + 44 + (i + 1) * 32) else { break };
            let end = name.iter().position(|&c| c == 0).unwrap_or(32);
            if end == 0 {
                break;
            }
            methods.push(String::from_utf8_lossy(&name[..end]).into_owned());
        }

        f.seek(SeekFrom::Start(index_offset))?;
        let mut idx = vec![0u8; index_size];
        f.read_exact(&mut idx).context("pak: reading index")?;
        let mut c = 0usize;
        let mount_point = fstring(&idx, &mut c)?;
        c += 4; // entry count
        c += 8; // path hash seed
        let has_phi = u32_at(&idx, c)?;
        c += 4;
        if has_phi != 0 {
            c += 16 + 20;
        }
        let has_fdi = u32_at(&idx, c)?;
        c += 4;
        if has_fdi == 0 {
            bail!("pak: no full directory index");
        }
        let fdi_offset = i64_at(&idx, c)? as u64;
        let fdi_size = i64_at(&idx, c + 8)? as usize;
        c += 16 + 20;
        let enc_size = u32_at(&idx, c)? as usize;
        c += 4;
        let enc = idx.get(c..c + enc_size).context("pak: truncated encoded entries")?.to_vec();

        f.seek(SeekFrom::Start(fdi_offset))?;
        let mut fdi = vec![0u8; fdi_size];
        f.read_exact(&mut fdi).context("pak: reading directory index")?;
        let mut c = 0usize;
        let dir_count = u32_at(&fdi, c)?;
        c += 4;
        let mut entries = HashMap::new();
        let mut file_count = 0usize;
        for _ in 0..dir_count {
            let dir = fstring(&fdi, &mut c)?;
            let dir_files = u32_at(&fdi, c)?;
            c += 4;
            let rel_dir = dir.split_once("WwiseAudio/").map(|(_, r)| r.to_string());
            for _ in 0..dir_files {
                let name = fstring(&fdi, &mut c)?;
                let entry_offset = u32_at(&fdi, c)? as i32;
                c += 4;
                file_count += 1;
                if let Some(rel_dir) = &rel_dir {
                    let rel = format!("{rel_dir}{name}");
                    if entry_offset >= 0 && keep(&rel) {
                        entries.insert(rel, decode_entry(&enc, entry_offset as usize)?);
                    }
                }
            }
        }
        Ok(PakIndex { version, mount_point, methods, entries, file_count, index_hash })
    }

    /// Uncompressed size of a file under `WwiseAudio/` (if it passed the filter).
    pub fn wwise_file_size(&self, rel: &str) -> Option<u64> {
        self.entries.get(rel).map(|e| e.uncompressed_size)
    }

    pub fn method_name(&self, entry: &PakEntry) -> &str {
        match entry.method {
            0 => "None",
            m => self.methods.get(m as usize - 1).map(String::as_str).unwrap_or("Unknown"),
        }
    }

    /// Read up to `max_bytes` from the start of an entry (whole blocks are
    /// decompressed until enough bytes are available).
    pub fn read_entry_prefix<R: Read + Seek>(&self, pak: &mut R, entry: &PakEntry, dec: &mut dyn Decompressor, max_bytes: usize) -> Result<Vec<u8>> {
        if entry.encrypted {
            bail!("pak: entry is encrypted");
        }
        let want = (entry.uncompressed_size as usize).min(max_bytes);
        if entry.method == 0 {
            pak.seek(SeekFrom::Start(entry.offset + entry.header_size))?;
            let mut out = vec![0u8; want];
            pak.read_exact(&mut out).context("pak: reading entry")?;
            return Ok(out);
        }
        let method = self.method_name(entry).to_string();
        let mut out = Vec::with_capacity(want.max(entry.block_size as usize));
        let mut remaining = entry.uncompressed_size;
        let mut compressed = Vec::new();
        let mut raw = Vec::new();
        for (i, &(start, end)) in entry.blocks.iter().enumerate() {
            if out.len() >= want || remaining == 0 {
                break;
            }
            compressed.clear();
            compressed.resize((end - start) as usize, 0);
            pak.seek(SeekFrom::Start(entry.offset + start))?;
            pak.read_exact(&mut compressed).with_context(|| format!("pak: reading block {i}"))?;
            let block = entry.block_size.min(remaining) as usize;
            raw.clear();
            raw.resize(block, 0);
            dec.decompress(&method, &compressed, &mut raw).with_context(|| format!("pak: decompressing block {i}"))?;
            out.extend_from_slice(&raw);
            remaining -= block as u64;
        }
        out.truncate(want);
        Ok(out)
    }

    /// Read and decompress one entry.
    pub fn read_entry<R: Read + Seek>(&self, pak: &mut R, entry: &PakEntry, dec: &mut dyn Decompressor) -> Result<Vec<u8>> {
        if entry.encrypted {
            bail!("pak: entry is encrypted");
        }
        if entry.method == 0 {
            pak.seek(SeekFrom::Start(entry.offset + entry.header_size))?;
            let mut out = vec![0u8; entry.uncompressed_size as usize];
            pak.read_exact(&mut out).context("pak: reading entry")?;
            return Ok(out);
        }
        let method = self.method_name(entry).to_string();
        let mut out = Vec::with_capacity(entry.uncompressed_size as usize);
        let mut remaining = entry.uncompressed_size;
        let mut compressed = Vec::new();
        let mut raw = Vec::new();
        for (i, &(start, end)) in entry.blocks.iter().enumerate() {
            if remaining == 0 {
                break;
            }
            compressed.clear();
            compressed.resize((end - start) as usize, 0);
            pak.seek(SeekFrom::Start(entry.offset + start))?;
            pak.read_exact(&mut compressed).with_context(|| format!("pak: reading block {i}"))?;
            let want = entry.block_size.min(remaining) as usize;
            raw.clear();
            raw.resize(want, 0);
            dec.decompress(&method, &compressed, &mut raw).with_context(|| format!("pak: decompressing block {i}"))?;
            out.extend_from_slice(&raw);
            remaining -= want as u64;
        }
        if remaining != 0 {
            bail!("pak: entry truncated ({} bytes missing)", remaining);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoDecompress;
    impl Decompressor for NoDecompress {
        fn decompress(&mut self, method: &str, input: &[u8], output: &mut [u8]) -> Result<()> {
            assert_eq!(method, "Fake");
            output.copy_from_slice(input);
            Ok(())
        }
    }

    #[test]
    fn decodes_raw_and_compressed_entries() {
        // Raw entry: offset 32-bit, uncompressed 32-bit, block size 2 << 11.
        let mut enc = ((1u32 << 31) | (1 << 30) | 2).to_le_bytes().to_vec();
        enc.extend_from_slice(&100u32.to_le_bytes());
        enc.extend_from_slice(&1234u32.to_le_bytes());
        let e = decode_entry(&enc, 0).unwrap();
        assert_eq!((e.offset, e.uncompressed_size, e.size, e.method, e.header_size), (100, 1234, 1234, 0, 53));
        assert!(e.blocks.is_empty());

        // Compressed entry, method 1, 2 blocks of 0x10000, 64-bit fields.
        let flags: u32 = (1 << 23) | (2 << 6) | 32; // block size 32 << 11 = 0x10000
        let mut enc = flags.to_le_bytes().to_vec();
        enc.extend_from_slice(&1000i64.to_le_bytes());
        enc.extend_from_slice(&100_000i64.to_le_bytes());
        enc.extend_from_slice(&50_000i64.to_le_bytes());
        enc.extend_from_slice(&30_000u32.to_le_bytes());
        enc.extend_from_slice(&20_000u32.to_le_bytes());
        let e = decode_entry(&enc, 0).unwrap();
        assert_eq!(e.header_size, 53 + 4 + 32);
        assert_eq!(e.block_size, 0x10000);
        assert_eq!(e.blocks, vec![(89, 30_089), (30_089, 50_089)]);

        // Single block: block size becomes the uncompressed size.
        let flags: u32 = (1 << 23) | (1 << 6) | (1 << 31) | (1 << 30) | (1 << 29);
        let mut enc = flags.to_le_bytes().to_vec();
        enc.extend_from_slice(&7u32.to_le_bytes());
        enc.extend_from_slice(&500u32.to_le_bytes());
        enc.extend_from_slice(&300u32.to_le_bytes());
        let e = decode_entry(&enc, 0).unwrap();
        assert_eq!(e.blocks, vec![(73, 373)]);
        assert_eq!(e.block_size, 500);
    }

    #[test]
    fn reads_compressed_entry_through_decompressor() {
        let index = PakIndex { version: 11, mount_point: "../../../".into(), methods: vec!["Fake".into()], entries: HashMap::new(), file_count: 0, index_hash: [0; 20] };
        // Pak bytes: 10 bytes of junk header, then two "compressed" blocks (stored verbatim).
        let mut pak = vec![0u8; 10];
        pak.extend_from_slice(b"hello wor");
        pak.extend_from_slice(b"ld!");
        let entry = PakEntry { offset: 0, uncompressed_size: 12, size: 12, method: 1, encrypted: false, block_size: 9, blocks: vec![(10, 19), (19, 22)], header_size: 10 };
        let out = index.read_entry(&mut std::io::Cursor::new(pak.clone()), &entry, &mut NoDecompress).unwrap();
        assert_eq!(out, b"hello world!");
        let head = index.read_entry_prefix(&mut std::io::Cursor::new(pak), &entry, &mut NoDecompress, 4).unwrap();
        assert_eq!(head, b"hell");
    }

    #[test]
    fn fstrings() {
        let mut b = 4u32.to_le_bytes().to_vec();
        b.extend_from_slice(b"abc\0");
        let mut c = 0;
        assert_eq!(fstring(&b, &mut c).unwrap(), "abc");
        assert_eq!(c, 8);
    }
}
