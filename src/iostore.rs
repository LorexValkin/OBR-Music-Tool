#![allow(dead_code, unused_variables)]

use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

const TOC_MAGIC: &[u8; 16] = b"-==--==--==--==-";
const TOC_HEADER_SIZE: usize = 144;

fn read_u32(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap())
}

fn read_i32(data: &[u8], offset: usize) -> i32 {
    i32::from_le_bytes(data[offset..offset + 4].try_into().unwrap())
}

struct ChunkOffsetLength {
    offset: u64,
    length: u64,
}

fn decode_offset_length(data: &[u8]) -> ChunkOffsetLength {
    let offset = u64::from_be_bytes([0, 0, 0, data[0], data[1], data[2], data[3], data[4]]);
    let length = u64::from_be_bytes([0, 0, 0, data[5], data[6], data[7], data[8], data[9]]);
    ChunkOffsetLength { offset, length }
}

struct CompressedBlock {
    compressed_offset: u64,
    compressed_size: u32,
    uncompressed_size: u32,
    compression_method: u8,
}

fn decode_compressed_block(data: &[u8]) -> CompressedBlock {
    let compressed_offset = u64::from_le_bytes([data[0], data[1], data[2], data[3], data[4], 0, 0, 0]);
    let compressed_size = u32::from_le_bytes([data[5], data[6], data[7], 0]);
    let uncompressed_size = u32::from_le_bytes([data[8], data[9], data[10], 0]);
    let compression_method = data[11];

    CompressedBlock {
        compressed_offset,
        compressed_size,
        uncompressed_size,
        compression_method,
    }
}

const INVALID_INDEX: u32 = u32::MAX;

fn read_fstring(data: &[u8], cursor: &mut usize) -> Result<String> {
    if *cursor + 4 > data.len() {
        bail!("fstring: not enough data for length");
    }
    let len = read_i32(data, *cursor);
    *cursor += 4;

    if len == 0 {
        return Ok(String::new());
    }

    if len > 0 {
        let byte_len = len as usize;
        if *cursor + byte_len > data.len() {
            bail!("fstring: not enough data for {} bytes", byte_len);
        }
        let end = if data[*cursor + byte_len - 1] == 0 {
            byte_len - 1
        } else {
            byte_len
        };
        let s = String::from_utf8_lossy(&data[*cursor..*cursor + end]).to_string();
        *cursor += byte_len;
        Ok(s)
    } else {
        let char_count = (-len) as usize;
        let byte_len = char_count * 2;
        if *cursor + byte_len > data.len() {
            bail!("fstring: not enough data for {} UTF-16 chars", char_count);
        }
        let chars: Vec<u16> = (0..char_count)
            .map(|i| u16::from_le_bytes([data[*cursor + i * 2], data[*cursor + i * 2 + 1]]))
            .collect();
        *cursor += byte_len;
        let end = if chars.last() == Some(&0) {
            chars.len() - 1
        } else {
            chars.len()
        };
        Ok(String::from_utf16_lossy(&chars[..end]))
    }
}

fn read_tarray_u32(data: &[u8], cursor: &mut usize) -> Result<Vec<u32>> {
    if *cursor + 4 > data.len() {
        bail!("tarray: not enough data for count");
    }
    let count = read_u32(data, *cursor) as usize;
    *cursor += 4;
    if *cursor + count * 4 > data.len() {
        bail!("tarray: not enough data for {} u32s", count);
    }
    let mut result = Vec::with_capacity(count);
    for _ in 0..count {
        result.push(read_u32(data, *cursor));
        *cursor += 4;
    }
    Ok(result)
}

struct DirectoryEntry {
    name_index: u32,
    first_child: u32,
    next_sibling: u32,
    first_file: u32,
}

struct FileEntry {
    name_index: u32,
    next_file: u32,
    user_data: u32,
}

fn parse_directory_index(
    data: &[u8],
) -> Result<(String, Vec<DirectoryEntry>, Vec<FileEntry>, Vec<String>)> {
    let mut cursor = 0;

    let mount_point = read_fstring(data, &mut cursor)?;

    let dir_count = read_u32(data, cursor) as usize;
    cursor += 4;
    let mut dirs = Vec::with_capacity(dir_count);
    for _ in 0..dir_count {
        if cursor + 16 > data.len() {
            bail!("directory entry overflow");
        }
        dirs.push(DirectoryEntry {
            name_index: read_u32(data, cursor),
            first_child: read_u32(data, cursor + 4),
            next_sibling: read_u32(data, cursor + 8),
            first_file: read_u32(data, cursor + 12),
        });
        cursor += 16;
    }

    let file_count = read_u32(data, cursor) as usize;
    cursor += 4;
    let mut files = Vec::with_capacity(file_count);
    for _ in 0..file_count {
        if cursor + 12 > data.len() {
            bail!("file entry overflow");
        }
        files.push(FileEntry {
            name_index: read_u32(data, cursor),
            next_file: read_u32(data, cursor + 4),
            user_data: read_u32(data, cursor + 8),
        });
        cursor += 12;
    }

    let string_count = read_u32(data, cursor) as usize;
    cursor += 4;
    let mut strings = Vec::with_capacity(string_count);
    for _ in 0..string_count {
        strings.push(read_fstring(data, &mut cursor)?);
    }

    Ok((mount_point, dirs, files, strings))
}

fn walk_directory(
    dirs: &[DirectoryEntry],
    files: &[FileEntry],
    strings: &[String],
    dir_index: u32,
    prefix: &str,
    result: &mut HashMap<String, u32>,
) {
    if dir_index == INVALID_INDEX || dir_index as usize >= dirs.len() {
        return;
    }
    let dir = &dirs[dir_index as usize];
    let dir_name = if dir.name_index < strings.len() as u32 {
        &strings[dir.name_index as usize]
    } else {
        ""
    };

    let path = if prefix.is_empty() {
        dir_name.to_string()
    } else if dir_name.is_empty() {
        prefix.to_string()
    } else {
        format!("{}/{}", prefix, dir_name)
    };

    let mut file_idx = dir.first_file;
    while file_idx != INVALID_INDEX && (file_idx as usize) < files.len() {
        let file = &files[file_idx as usize];
        let file_name = if file.name_index < strings.len() as u32 {
            &strings[file.name_index as usize]
        } else {
            ""
        };
        let file_path = if path.is_empty() {
            file_name.to_string()
        } else {
            format!("{}/{}", path, file_name)
        };
        result.insert(file_path, file.user_data);
        file_idx = file.next_file;
    }

    let mut child = dir.first_child;
    while child != INVALID_INDEX && (child as usize) < dirs.len() {
        walk_directory(dirs, files, strings, child, &path, result);
        child = dirs[child as usize].next_sibling;
    }
}

pub fn extract_files_from_iostore(
    utoc_path: &Path,
    path_filter: &str,
    extension_filter: &str,
    output_dir: &Path,
) -> Result<Vec<(String, PathBuf)>> {
    let toc_data = fs::read(utoc_path)
        .with_context(|| format!("reading {}", utoc_path.display()))?;

    if toc_data.len() < TOC_HEADER_SIZE {
        bail!("UTOC too small: {} bytes", toc_data.len());
    }
    if &toc_data[0..16] != TOC_MAGIC {
        bail!("not a valid UTOC file (bad magic)");
    }

    let version = toc_data[16];
    let entry_count = read_u32(&toc_data, 24) as usize;
    let compressed_block_count = read_u32(&toc_data, 28) as usize;
    let compressed_block_entry_size = read_u32(&toc_data, 32) as usize;
    let compression_method_count = read_u32(&toc_data, 36) as usize;
    let compression_method_length = read_u32(&toc_data, 40) as usize;
    let _compression_block_size = read_u32(&toc_data, 44);
    let directory_index_size = read_u32(&toc_data, 48) as usize;
    let partition_count = read_u32(&toc_data, 52) as usize;
    let container_flags = toc_data[80];
    let perfect_hash_seeds_count = read_u32(&toc_data, 84) as usize;
    let chunks_without_perfect_hash_count = if version >= 5 {
        read_u32(&toc_data, 96) as usize
    } else {
        0
    };

    let is_encrypted = (container_flags & 0x02) != 0;
    if is_encrypted {
        bail!("encrypted IoStore container — cannot extract");
    }

    let chunk_id_size = 12;
    let offset_length_size = 10;

    let mut pos = TOC_HEADER_SIZE;

    let chunk_ids_start = pos;
    pos += entry_count * chunk_id_size;

    let offsets_start = pos;
    pos += entry_count * offset_length_size;

    // Perfect hash seeds (version >= 4)
    if version >= 4 {
        pos += perfect_hash_seeds_count * 4;
    }
    // Chunks without perfect hash (version >= 5)
    if version >= 5 {
        pos += chunks_without_perfect_hash_count * 4;
    }

    let blocks_start = pos;
    let actual_block_entry_size = if compressed_block_entry_size > 0 {
        compressed_block_entry_size
    } else {
        12
    };
    pos += compressed_block_count * actual_block_entry_size;

    pos += compression_method_count * compression_method_length;

    // Skip signature block if signed (flag 0x01)
    let is_signed = (container_flags & 0x01) != 0;
    if is_signed {
        // hash_size (4) + toc_signature (hash_size) + block_signature (hash_size)
        // + chunk_block_signatures (entry_count * hash_size)
        if pos + 4 <= toc_data.len() {
            let hash_size = read_u32(&toc_data, pos) as usize;
            pos += 4;
            pos += hash_size; // toc signature
            pos += hash_size; // block signature
            pos += compressed_block_count * hash_size; // per-block signatures
        }
    }

    if pos + directory_index_size > toc_data.len() {
        bail!(
            "directory index extends past UTOC end: offset {} + size {} > file {}",
            pos, directory_index_size, toc_data.len()
        );
    }
    let dir_index_data = &toc_data[pos..pos + directory_index_size];

    let (_mount_point, dirs, files, strings) = parse_directory_index(dir_index_data)?;

    let mut file_map = HashMap::new();
    if !dirs.is_empty() {
        walk_directory(&dirs, &files, &strings, 0, "", &mut file_map);
    }

    let filter_lower = path_filter.to_lowercase();
    let ext_lower = extension_filter.to_lowercase();

    let matching: Vec<(String, u32)> = file_map
        .into_iter()
        .filter(|(path, _)| {
            let lower = path.to_lowercase().replace('\\', "/");
            lower.contains(&filter_lower) && lower.ends_with(&ext_lower)
        })
        .collect();

    if matching.is_empty() {
        return Ok(Vec::new());
    }

    let ucas_path = utoc_path.with_extension("ucas");
    if !ucas_path.is_file() {
        bail!("UCAS file not found: {}", ucas_path.display());
    }
    let mut ucas = fs::File::open(&ucas_path)
        .with_context(|| format!("opening {}", ucas_path.display()))?;

    let compression_names: Vec<String> = {
        let names_start = blocks_start + compressed_block_count * actual_block_entry_size;
        (0..compression_method_count)
            .map(|i| {
                let start = names_start + i * compression_method_length;
                let end = start + compression_method_length;
                let slice = &toc_data[start..end.min(toc_data.len())];
                let nul = slice.iter().position(|&b| b == 0).unwrap_or(slice.len());
                String::from_utf8_lossy(&slice[..nul]).to_string()
            })
            .collect()
    };

    let compression_block_size = _compression_block_size as u64;

    let mut extracted = Vec::new();
    fs::create_dir_all(output_dir)?;

    for (file_path, chunk_index) in &matching {
        if (*chunk_index as usize) >= entry_count {
            continue;
        }

        let ol_offset = offsets_start + (*chunk_index as usize) * offset_length_size;
        let ol = decode_offset_length(&toc_data[ol_offset..ol_offset + offset_length_size]);

        let relative = file_path
            .trim_start_matches('/')
            .replace('/', "\\");
        let dest = output_dir.join(&relative);
        if dest.is_file() {
            extracted.push((file_path.clone(), dest));
            continue;
        }
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }

        if compression_block_size == 0 || compressed_block_count == 0 {
            continue;
        }

        let first_block = (ol.offset / compression_block_size) as usize;
        let last_block = ((ol.offset + ol.length - 1) / compression_block_size) as usize;

        let mut output_data = Vec::with_capacity(ol.length as usize);
        let mut any_compressed = false;

        for block_idx in first_block..=last_block {
            if block_idx >= compressed_block_count {
                break;
            }
            let block_data_offset = blocks_start + block_idx * actual_block_entry_size;
            if block_data_offset + actual_block_entry_size > toc_data.len() {
                break;
            }
            let block = decode_compressed_block(
                &toc_data[block_data_offset..block_data_offset + actual_block_entry_size],
            );

            if block.compression_method != 0 {
                any_compressed = true;
                break;
            }

            let mut block_data = vec![0u8; block.compressed_size as usize];
            ucas.seek(SeekFrom::Start(block.compressed_offset))?;
            ucas.read_exact(&mut block_data)?;
            output_data.extend_from_slice(&block_data);
        }

        if any_compressed {
            continue;
        }

        // Trim to the exact file bounds within the blocks
        let start_within = (ol.offset % compression_block_size) as usize;
        if start_within + ol.length as usize <= output_data.len() {
            let file_data = &output_data[start_within..start_within + ol.length as usize];
            fs::write(&dest, file_data)?;
            extracted.push((file_path.clone(), dest));
        }
    }

    Ok(extracted)
}
