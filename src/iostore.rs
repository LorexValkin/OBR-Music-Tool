//! Minimal reader for Unreal Engine 5 IoStore containers (`.utoc` + `.ucas`).
//!
//! This file is shared between the application and the offline index builder
//! (`tools/sfxindex`, which includes it via `#[path]`), so it must only depend on
//! `std` and `anyhow`. Decompression is injected through the [`Decompressor`]
//! trait: the builder supplies an Oodle implementation, the app never needs one.

use anyhow::{bail, Context, Result};
use std::io::{Read, Seek, SeekFrom};
use std::ops::Range;
use std::path::Path;

const TOC_MAGIC: &[u8; 16] = b"-==--==--==--==-";
const TOC_HEADER_SIZE: usize = 144;
const INVALID_INDEX: u32 = u32::MAX;

/// Container flag bits (`EIoContainerFlags`).
#[cfg_attr(not(test), allow(dead_code))]
pub const FLAG_COMPRESSED: u8 = 1;
pub const FLAG_ENCRYPTED: u8 = 2;
pub const FLAG_SIGNED: u8 = 4;
#[cfg_attr(not(test), allow(dead_code))]
pub const FLAG_INDEXED: u8 = 8;

/// One 12-byte compression block entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompressedBlock {
    /// Absolute offset in the (virtual, partition-spanning) `.ucas` space.
    pub offset: u64,
    pub compressed_size: u32,
    pub uncompressed_size: u32,
    /// 0 = stored raw, otherwise `1 + index` into [`Utoc::methods`].
    pub method: u8,
}

/// Supplies block decompression for [`Utoc::read_chunk`].
pub trait Decompressor {
    /// Decompress `input` (compressed with `method`, e.g. `"Oodle"`) into
    /// `output`, which is exactly the uncompressed size.
    fn decompress(&mut self, method: &str, input: &[u8], output: &mut [u8]) -> Result<()>;
}

/// Parsed table of contents of an IoStore container.
#[derive(Debug)]
pub struct Utoc {
    pub version: u8,
    pub container_flags: u8,
    pub compression_block_size: u32,
    pub partition_count: u32,
    pub partition_size: u64,
    /// Compression method names; block method `m >= 1` maps to `methods[m - 1]`.
    pub methods: Vec<String>,
    pub mount_point: String,
    /// `(path relative to the mount point, chunk index)`, sorted by path.
    pub files: Vec<(String, u32)>,
    chunk_spans: Vec<(u64, u64)>,
    blocks: Vec<CompressedBlock>,
    header: [u8; TOC_HEADER_SIZE],
    directory_index: Vec<u8>,
}

fn u32_at(data: &[u8], offset: usize) -> Result<u32> {
    let bytes = data
        .get(offset..offset + 4)
        .with_context(|| format!("utoc: truncated at offset {offset}"))?;
    Ok(u32::from_le_bytes(bytes.try_into().unwrap()))
}

fn u64_at(data: &[u8], offset: usize) -> Result<u64> {
    let bytes = data
        .get(offset..offset + 8)
        .with_context(|| format!("utoc: truncated at offset {offset}"))?;
    Ok(u64::from_le_bytes(bytes.try_into().unwrap()))
}

fn read_fstring(data: &[u8], cursor: &mut usize) -> Result<String> {
    let len = u32_at(data, *cursor)? as i32;
    *cursor += 4;
    if len == 0 {
        return Ok(String::new());
    }
    if len > 0 {
        let byte_len = len as usize;
        let bytes = data
            .get(*cursor..*cursor + byte_len)
            .context("utoc: truncated string")?;
        *cursor += byte_len;
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(byte_len);
        Ok(String::from_utf8_lossy(&bytes[..end]).into_owned())
    } else {
        let char_count = (-(len as i64)) as usize;
        let byte_len = char_count * 2;
        let bytes = data
            .get(*cursor..*cursor + byte_len)
            .context("utoc: truncated wide string")?;
        *cursor += byte_len;
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .take_while(|&u| u != 0)
            .collect();
        Ok(String::from_utf16_lossy(&units))
    }
}

struct DirectoryEntry {
    name: u32,
    first_child: u32,
    next_sibling: u32,
    first_file: u32,
}

struct FileEntry {
    name: u32,
    next_file: u32,
    user_data: u32,
}

fn parse_directory_index(data: &[u8]) -> Result<(String, Vec<(String, u32)>)> {
    let mut cursor = 0;
    let mount_point = read_fstring(data, &mut cursor)?;

    let dir_count = u32_at(data, cursor)? as usize;
    cursor += 4;
    let mut dirs = Vec::with_capacity(dir_count);
    for _ in 0..dir_count {
        dirs.push(DirectoryEntry {
            name: u32_at(data, cursor)?,
            first_child: u32_at(data, cursor + 4)?,
            next_sibling: u32_at(data, cursor + 8)?,
            first_file: u32_at(data, cursor + 12)?,
        });
        cursor += 16;
    }

    let file_count = u32_at(data, cursor)? as usize;
    cursor += 4;
    let mut files = Vec::with_capacity(file_count);
    for _ in 0..file_count {
        files.push(FileEntry {
            name: u32_at(data, cursor)?,
            next_file: u32_at(data, cursor + 4)?,
            user_data: u32_at(data, cursor + 8)?,
        });
        cursor += 12;
    }

    let string_count = u32_at(data, cursor)? as usize;
    cursor += 4;
    let mut strings = Vec::with_capacity(string_count);
    for _ in 0..string_count {
        strings.push(read_fstring(data, &mut cursor)?);
    }

    let name = |idx: u32| -> &str {
        strings.get(idx as usize).map(String::as_str).unwrap_or("")
    };

    // Iterative walk (the tree is shallow, but avoid recursion on untrusted input).
    let mut out = Vec::with_capacity(file_count);
    let mut stack: Vec<(u32, String)> = vec![(0, String::new())];
    let mut visited = 0usize;
    while let Some((dir_idx, prefix)) = stack.pop() {
        if dir_idx == INVALID_INDEX || dir_idx as usize >= dirs.len() {
            continue;
        }
        visited += 1;
        if visited > dirs.len() {
            bail!("utoc: directory index contains a cycle");
        }
        let dir = &dirs[dir_idx as usize];
        let dir_name = name(dir.name);
        let path = match (prefix.is_empty(), dir_name.is_empty()) {
            (true, _) => dir_name.to_string(),
            (false, true) => prefix.clone(),
            (false, false) => format!("{prefix}/{dir_name}"),
        };

        let mut file_idx = dir.first_file;
        let mut guard = 0usize;
        while file_idx != INVALID_INDEX && (file_idx as usize) < files.len() {
            guard += 1;
            if guard > files.len() {
                bail!("utoc: file list contains a cycle");
            }
            let file = &files[file_idx as usize];
            let file_name = name(file.name);
            let file_path = if path.is_empty() {
                file_name.to_string()
            } else {
                format!("{path}/{file_name}")
            };
            out.push((file_path, file.user_data));
            file_idx = file.next_file;
        }

        let mut child = dir.first_child;
        let mut guard = 0usize;
        while child != INVALID_INDEX && (child as usize) < dirs.len() {
            guard += 1;
            if guard > dirs.len() {
                bail!("utoc: sibling list contains a cycle");
            }
            stack.push((child, path.clone()));
            child = dirs[child as usize].next_sibling;
        }
    }
    out.sort();
    Ok((mount_point, out))
}

/// FNV-1a 64-bit hash (stable across Rust versions, unlike `DefaultHasher`).
pub fn fnv1a64(parts: &[&[u8]]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for part in parts {
        for &b in *part {
            hash ^= b as u64;
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    hash
}

impl Utoc {
    /// Parse a `.utoc` file that has been read into memory.
    pub fn parse(toc: &[u8]) -> Result<Utoc> {
        if toc.len() < TOC_HEADER_SIZE {
            bail!("utoc: file too small ({} bytes)", toc.len());
        }
        if &toc[..16] != TOC_MAGIC {
            bail!("utoc: bad magic");
        }
        let version = toc[16];
        if version < 3 {
            bail!("utoc: unsupported version {version}");
        }
        let entry_count = u32_at(toc, 24)? as usize;
        let block_count = u32_at(toc, 28)? as usize;
        let block_entry_size = match u32_at(toc, 32)? as usize {
            0 => 12,
            n => n,
        };
        let method_count = u32_at(toc, 36)? as usize;
        let method_length = u32_at(toc, 40)? as usize;
        let compression_block_size = u32_at(toc, 44)?;
        let directory_index_size = u32_at(toc, 48)? as usize;
        let partition_count = u32_at(toc, 52)?.max(1);
        let container_flags = toc[80];
        let perfect_hash_seeds = u32_at(toc, 84)? as usize;
        let partition_size = u64_at(toc, 88)?;
        let chunks_without_perfect_hash = if version >= 5 {
            u32_at(toc, 96)? as usize
        } else {
            0
        };

        if container_flags & FLAG_ENCRYPTED != 0 {
            bail!("utoc: container is encrypted");
        }

        let mut pos = TOC_HEADER_SIZE;
        pos += entry_count * 12; // chunk ids
        let offsets_start = pos;
        pos += entry_count * 10;
        if version >= 4 {
            pos += perfect_hash_seeds * 4;
        }
        if version >= 5 {
            pos += chunks_without_perfect_hash * 4;
        }
        let blocks_start = pos;
        pos += block_count * block_entry_size;
        let methods_start = pos;
        pos += method_count * method_length;
        if container_flags & FLAG_SIGNED != 0 {
            let hash_size = u32_at(toc, pos)? as usize;
            pos += 4 + hash_size * 2 + block_count * hash_size;
        }
        let dir_start = pos;
        let directory_index = toc
            .get(dir_start..dir_start + directory_index_size)
            .context("utoc: directory index extends past end of file")?
            .to_vec();

        let mut chunk_spans = Vec::with_capacity(entry_count);
        for i in 0..entry_count {
            let o = offsets_start + i * 10;
            let b = toc.get(o..o + 10).context("utoc: truncated offset table")?;
            let offset = u64::from_be_bytes([0, 0, 0, b[0], b[1], b[2], b[3], b[4]]);
            let length = u64::from_be_bytes([0, 0, 0, b[5], b[6], b[7], b[8], b[9]]);
            chunk_spans.push((offset, length));
        }

        let mut blocks = Vec::with_capacity(block_count);
        for i in 0..block_count {
            let o = blocks_start + i * block_entry_size;
            let b = toc.get(o..o + 12).context("utoc: truncated block table")?;
            blocks.push(CompressedBlock {
                offset: u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], 0, 0, 0]),
                compressed_size: u32::from_le_bytes([b[5], b[6], b[7], 0]),
                uncompressed_size: u32::from_le_bytes([b[8], b[9], b[10], 0]),
                method: b[11],
            });
        }

        let mut methods = Vec::with_capacity(method_count);
        for i in 0..method_count {
            let o = methods_start + i * method_length;
            let b = toc.get(o..o + method_length).context("utoc: truncated method table")?;
            let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
            methods.push(String::from_utf8_lossy(&b[..end]).into_owned());
        }

        let (mount_point, files) = if directory_index_size > 0 {
            parse_directory_index(&directory_index)?
        } else {
            (String::new(), Vec::new())
        };

        let mut header = [0u8; TOC_HEADER_SIZE];
        header.copy_from_slice(&toc[..TOC_HEADER_SIZE]);

        Ok(Utoc {
            version,
            container_flags,
            compression_block_size,
            partition_count,
            partition_size,
            methods,
            mount_point,
            files,
            chunk_spans,
            blocks,
            header,
            directory_index,
        })
    }

    /// Read and parse a `.utoc` file from disk.
    pub fn open(utoc_path: &Path) -> Result<Utoc> {
        let data = std::fs::read(utoc_path)
            .with_context(|| format!("reading {}", utoc_path.display()))?;
        Utoc::parse(&data).with_context(|| format!("parsing {}", utoc_path.display()))
    }

    pub fn chunk_count(&self) -> usize {
        self.chunk_spans.len()
    }

    /// `(offset, length)` of a chunk in the virtual `.ucas` space.
    pub fn chunk_span(&self, chunk: u32) -> Option<(u64, u64)> {
        self.chunk_spans.get(chunk as usize).copied()
    }

    /// Range of compression-block indices covering a chunk.
    pub fn chunk_blocks(&self, chunk: u32) -> Option<Range<usize>> {
        let (offset, length) = self.chunk_span(chunk)?;
        let bs = self.compression_block_size as u64;
        if bs == 0 {
            return None;
        }
        let first = (offset / bs) as usize;
        let last = ((offset + length).max(offset + 1) - 1) / bs;
        let end = (last as usize + 1).min(self.blocks.len());
        Some(first.min(end)..end)
    }

    /// Name of the compression method used by a block (`"None"` for raw blocks).
    pub fn method_name(&self, block: &CompressedBlock) -> &str {
        match block.method {
            0 => "None",
            m => self
                .methods
                .get(m as usize - 1)
                .map(String::as_str)
                .unwrap_or("Unknown"),
        }
    }

    /// True when every block of the chunk is stored raw (no decompression needed).
    pub fn chunk_is_raw(&self, chunk: u32) -> bool {
        match self.chunk_blocks(chunk) {
            Some(range) => self.blocks[range].iter().all(|b| b.method == 0),
            None => false,
        }
    }

    /// Read a whole chunk from partition 0 of the `.ucas`, decompressing blocks
    /// through `dec` where necessary.
    pub fn read_chunk<R: Read + Seek>(
        &self,
        ucas: &mut R,
        chunk: u32,
        dec: &mut dyn Decompressor,
    ) -> Result<Vec<u8>> {
        let (offset, length) = self
            .chunk_span(chunk)
            .with_context(|| format!("utoc: chunk {chunk} out of range"))?;
        let range = self
            .chunk_blocks(chunk)
            .with_context(|| format!("utoc: chunk {chunk} has no blocks"))?;
        let bs = self.compression_block_size as u64;
        let mut skip = (offset % bs) as usize;
        let mut out = Vec::with_capacity(length as usize);
        let mut compressed = Vec::new();
        let mut raw = Vec::new();

        for block_idx in range {
            if out.len() as u64 >= length {
                break;
            }
            let block = self.blocks[block_idx];
            if self.partition_count > 1 && self.partition_size > 0 && block.offset >= self.partition_size {
                bail!("utoc: multi-partition containers are not supported (block in partition {})",
                    block.offset / self.partition_size);
            }
            compressed.clear();
            compressed.resize(block.compressed_size as usize, 0);
            ucas.seek(SeekFrom::Start(block.offset))
                .with_context(|| format!("seeking to block {block_idx}"))?;
            ucas.read_exact(&mut compressed)
                .with_context(|| format!("reading block {block_idx}"))?;

            let data: &[u8] = if block.method == 0 {
                &compressed
            } else {
                raw.clear();
                raw.resize(block.uncompressed_size as usize, 0);
                let method = self.method_name(&block).to_string();
                dec.decompress(&method, &compressed, &mut raw)
                    .with_context(|| format!("decompressing block {block_idx} ({method})"))?;
                &raw
            };

            let start = skip.min(data.len());
            skip = 0;
            let remaining = (length - out.len() as u64) as usize;
            let take = (data.len() - start).min(remaining);
            out.extend_from_slice(&data[start..start + take]);
        }

        if out.len() as u64 != length {
            bail!("utoc: chunk {chunk} is truncated ({} of {} bytes)", out.len(), length);
        }
        Ok(out)
    }

    /// Stable fingerprint of the container layout (header + directory index).
    pub fn fingerprint(&self) -> u64 {
        fnv1a64(&[&self.header, &self.directory_index])
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Cursor;

    /// Build a synthetic v5 container. `chunks` are raw payloads; every block
    /// whose index is in `fake_blocks` is stored byte-reversed with method 1 ("Fake").
    pub(crate) fn synthetic_container(
        files: &[(&str, u32)],
        chunks: &[Vec<u8>],
        block_size: u32,
        fake_blocks: &[usize],
        flags: u8,
    ) -> (Vec<u8>, Vec<u8>) {
        let mut ucas = Vec::new();
        let mut spans = Vec::new();
        let mut blocks: Vec<CompressedBlock> = Vec::new();
        let mut virtual_offset = 0u64;
        for chunk in chunks {
            spans.push((virtual_offset, chunk.len() as u64));
            for piece in chunk.chunks(block_size as usize) {
                let idx = blocks.len();
                let stored: Vec<u8> = if fake_blocks.contains(&idx) {
                    piece.iter().rev().copied().collect()
                } else {
                    piece.to_vec()
                };
                blocks.push(CompressedBlock {
                    offset: ucas.len() as u64,
                    compressed_size: stored.len() as u32,
                    uncompressed_size: piece.len() as u32,
                    method: if fake_blocks.contains(&idx) { 1 } else { 0 },
                });
                ucas.extend_from_slice(&stored);
            }
            let blocks_used = (chunk.len() as u64 + block_size as u64 - 1) / block_size as u64;
            virtual_offset += blocks_used.max(1) * block_size as u64;
        }

        // Directory index: flat root dir + "sub" dir.
        let mut strings: Vec<String> = Vec::new();
        let mut intern = |s: &str| -> u32 {
            if let Some(i) = strings.iter().position(|x| x == s) {
                return i as u32;
            }
            strings.push(s.to_string());
            (strings.len() - 1) as u32
        };
        // dirs: 0 = root (name ""), 1 = "sub"
        let root_name = intern("");
        let sub_name = intern("sub");
        let mut file_entries: Vec<(u32, u32, u32)> = Vec::new(); // (name, next, user_data)
        let mut root_files = Vec::new();
        let mut sub_files = Vec::new();
        for (path, chunk) in files {
            match path.strip_prefix("sub/") {
                Some(rest) => sub_files.push((intern(rest), *chunk)),
                None => root_files.push((intern(path), *chunk)),
            }
        }
        let chain = |entries: &mut Vec<(u32, u32, u32)>, list: &[(u32, u32)]| -> u32 {
            if list.is_empty() {
                return INVALID_INDEX;
            }
            let first = entries.len() as u32;
            for (i, (name, chunk)) in list.iter().enumerate() {
                let next = if i + 1 < list.len() { entries.len() as u32 + 1 } else { INVALID_INDEX };
                entries.push((*name, next, *chunk));
            }
            first
        };
        let root_first = chain(&mut file_entries, &root_files);
        let sub_first = chain(&mut file_entries, &sub_files);

        let mut dir_index = Vec::new();
        let put_fstring = |buf: &mut Vec<u8>, s: &str| {
            if s.is_empty() {
                buf.extend_from_slice(&0u32.to_le_bytes());
            } else {
                buf.extend_from_slice(&((s.len() + 1) as u32).to_le_bytes());
                buf.extend_from_slice(s.as_bytes());
                buf.push(0);
            }
        };
        put_fstring(&mut dir_index, "../../../");
        dir_index.extend_from_slice(&2u32.to_le_bytes());
        for (name, child, sibling, first_file) in [
            (root_name, 1u32, INVALID_INDEX, root_first),
            (sub_name, INVALID_INDEX, INVALID_INDEX, sub_first),
        ] {
            for v in [name, child, sibling, first_file] {
                dir_index.extend_from_slice(&v.to_le_bytes());
            }
        }
        dir_index.extend_from_slice(&(file_entries.len() as u32).to_le_bytes());
        for (name, next, user) in &file_entries {
            for v in [name, next, user] {
                dir_index.extend_from_slice(&v.to_le_bytes());
            }
        }
        dir_index.extend_from_slice(&(strings.len() as u32).to_le_bytes());
        for s in &strings {
            put_fstring(&mut dir_index, s);
        }

        let methods = [b"Fake".to_vec()];
        let method_length = 32usize;
        let mut toc = Vec::new();
        toc.extend_from_slice(TOC_MAGIC);
        toc.push(5); // version
        toc.extend_from_slice(&[0, 0, 0]);
        toc.extend_from_slice(&(TOC_HEADER_SIZE as u32).to_le_bytes());
        toc.extend_from_slice(&(chunks.len() as u32).to_le_bytes());
        toc.extend_from_slice(&(blocks.len() as u32).to_le_bytes());
        toc.extend_from_slice(&12u32.to_le_bytes());
        toc.extend_from_slice(&(methods.len() as u32).to_le_bytes());
        toc.extend_from_slice(&(method_length as u32).to_le_bytes());
        toc.extend_from_slice(&block_size.to_le_bytes());
        toc.extend_from_slice(&(dir_index.len() as u32).to_le_bytes());
        toc.extend_from_slice(&1u32.to_le_bytes()); // partition count
        toc.extend_from_slice(&7u64.to_le_bytes()); // container id
        toc.extend_from_slice(&[0u8; 16]); // encryption key guid
        toc.push(flags);
        toc.extend_from_slice(&[0, 0, 0]);
        toc.extend_from_slice(&0u32.to_le_bytes()); // perfect hash seeds
        toc.extend_from_slice(&u64::MAX.to_le_bytes()); // partition size
        toc.extend_from_slice(&0u32.to_le_bytes()); // chunks without perfect hash
        toc.extend_from_slice(&0u32.to_le_bytes());
        toc.extend_from_slice(&[0u8; 40]);
        assert_eq!(toc.len(), TOC_HEADER_SIZE);

        for _ in chunks {
            toc.extend_from_slice(&[0u8; 12]); // chunk ids (unused)
        }
        for (offset, length) in &spans {
            toc.extend_from_slice(&offset.to_be_bytes()[3..]);
            toc.extend_from_slice(&length.to_be_bytes()[3..]);
        }
        for b in &blocks {
            toc.extend_from_slice(&b.offset.to_le_bytes()[..5]);
            toc.extend_from_slice(&b.compressed_size.to_le_bytes()[..3]);
            toc.extend_from_slice(&b.uncompressed_size.to_le_bytes()[..3]);
            toc.push(b.method);
        }
        for m in &methods {
            let mut name = m.clone();
            name.resize(method_length, 0);
            toc.extend_from_slice(&name);
        }
        if flags & FLAG_SIGNED != 0 {
            let hash_size = 20u32;
            toc.extend_from_slice(&hash_size.to_le_bytes());
            toc.extend_from_slice(&vec![0xAB; (hash_size as usize) * (2 + blocks.len())]);
        }
        toc.extend_from_slice(&dir_index);
        toc.extend_from_slice(&vec![0xEE; 33 * chunks.len()]); // metas (ignored)
        (toc, ucas)
    }

    struct Reverser;
    impl Decompressor for Reverser {
        fn decompress(&mut self, method: &str, input: &[u8], output: &mut [u8]) -> Result<()> {
            assert_eq!(method, "Fake");
            assert_eq!(input.len(), output.len());
            for (o, i) in output.iter_mut().zip(input.iter().rev()) {
                *o = *i;
            }
            Ok(())
        }
    }

    #[test]
    fn walks_directory_index_and_reads_chunks() {
        let chunks = vec![
            b"hello".to_vec(),
            (0..40u8).collect::<Vec<u8>>(),
            b"z".to_vec(),
        ];
        let (toc, ucas) = synthetic_container(
            &[("a.uasset", 0), ("sub/b.uasset", 1), ("sub/c.uasset", 2)],
            &chunks,
            16,
            &[2],
            FLAG_COMPRESSED | FLAG_INDEXED,
        );
        let utoc = Utoc::parse(&toc).unwrap();
        assert_eq!(utoc.version, 5);
        assert_eq!(utoc.mount_point, "../../../");
        assert_eq!(utoc.methods, vec!["Fake".to_string()]);
        assert_eq!(
            utoc.files,
            vec![
                ("a.uasset".to_string(), 0),
                ("sub/b.uasset".to_string(), 1),
                ("sub/c.uasset".to_string(), 2),
            ]
        );
        assert!(utoc.chunk_is_raw(0));
        assert!(!utoc.chunk_is_raw(1));

        let mut cursor = Cursor::new(ucas);
        let mut dec = Reverser;
        for (i, expected) in chunks.iter().enumerate() {
            let got = utoc.read_chunk(&mut cursor, i as u32, &mut dec).unwrap();
            assert_eq!(&got, expected, "chunk {i}");
        }
        assert!(utoc.read_chunk(&mut cursor, 9, &mut dec).is_err());
    }

    #[test]
    fn flags_are_interpreted_correctly() {
        let chunks = vec![b"data".to_vec()];
        let (toc, _) = synthetic_container(&[("x", 0)], &chunks, 16, &[], FLAG_ENCRYPTED);
        assert!(Utoc::parse(&toc).unwrap_err().to_string().contains("encrypted"));

        let (toc, ucas) = synthetic_container(&[("x", 0)], &chunks, 16, &[], FLAG_SIGNED | FLAG_INDEXED);
        let utoc = Utoc::parse(&toc).unwrap();
        assert_eq!(utoc.files, vec![("x".to_string(), 0)]);
        let mut dec = Reverser;
        assert_eq!(utoc.read_chunk(&mut Cursor::new(ucas), 0, &mut dec).unwrap(), b"data");

        // Bit 0 alone (Compressed) must not be mistaken for "signed".
        let (toc, _) = synthetic_container(&[("x", 0)], &chunks, 16, &[], FLAG_COMPRESSED);
        assert!(Utoc::parse(&toc).is_ok());
    }

    #[test]
    fn fingerprint_is_stable_and_sensitive() {
        let chunks = vec![b"data".to_vec()];
        let (toc_a, _) = synthetic_container(&[("x", 0)], &chunks, 16, &[], FLAG_INDEXED);
        let (toc_b, _) = synthetic_container(&[("y", 0)], &chunks, 16, &[], FLAG_INDEXED);
        let a = Utoc::parse(&toc_a).unwrap().fingerprint();
        let a2 = Utoc::parse(&toc_a).unwrap().fingerprint();
        let b = Utoc::parse(&toc_b).unwrap().fingerprint();
        assert_eq!(a, a2);
        assert_ne!(a, b);
        assert_eq!(fnv1a64(&[b""]), 0xcbf2_9ce4_8422_2325);
    }

    #[test]
    fn rejects_garbage() {
        assert!(Utoc::parse(b"nope").is_err());
        let mut bad = vec![0u8; 200];
        bad[..16].copy_from_slice(TOC_MAGIC);
        bad[16] = 5;
        bad[24] = 0xFF; // absurd entry count -> truncated tables
        bad[25] = 0xFF;
        assert!(Utoc::parse(&bad).is_err());
    }
}
