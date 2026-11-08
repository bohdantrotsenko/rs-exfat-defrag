use crate::structs::*;
use crate::volume::{Volume, chunk_count};
use crate::{Error, Result};

/// A file or directory whose cluster allocation is not a single contiguous run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FragmentedEntry {
    pub path: String,
    pub is_directory: bool,
    pub size: u64,
    pub chunks: usize,
    pub clusters: usize,
}

/// Walk the directory tree and collect files/directories with more than one cluster run.
pub fn analyze(vol: &Volume) -> Result<Vec<FragmentedEntry>> {
    let mut out = Vec::new();

    let root_first = vol.boot.first_cluster_of_root_directory;
    let root_chain = vol.cluster_chain(root_first, 0, false)?;
    let root_size = root_chain.len() as u64 * vol.cluster_size() as u64;
    let root_chunks = chunk_count(&root_chain);
    if root_chunks > 1 {
        out.push(FragmentedEntry {
            path: "/".to_string(),
            is_directory: true,
            size: root_size,
            chunks: root_chunks,
            clusters: root_chain.len(),
        });
    }

    let root_data = vol.read_root_directory()?;
    let mut stack: Vec<(String, Vec<u8>)> = vec![(String::new(), root_data)];

    while let Some((dir_path, data)) = stack.pop() {
        for record in parse_directory(&data)? {
            let chain = vol.cluster_chain(
                record.first_cluster(),
                record.data_length(),
                record.no_fat_chain(),
            )?;
            let chunks = chunk_count(&chain);
            let path = join_path(&dir_path, &record.name);
            if chunks > 1 {
                out.push(FragmentedEntry {
                    path: path.clone(),
                    is_directory: record.is_directory(),
                    size: record.data_length(),
                    chunks,
                    clusters: chain.len(),
                });
            }
            if record.is_directory() && record.first_cluster() != 0 {
                let child = vol.read_allocation(
                    record.first_cluster(),
                    record.data_length(),
                    record.no_fat_chain(),
                )?;
                stack.push((path, child));
            }
        }
    }

    Ok(out)
}

fn skip_set(off: usize, len: usize, secondary_count: u8) -> Result<usize> {
    let bytes = DIR_ENTRY_SIZE * (1 + secondary_count as usize);
    if off + bytes > len {
        return Err(Error::invalid("truncated directory entry set"));
    }
    Ok(off + bytes)
}

fn join_path(parent: &str, name: &str) -> String {
    if parent.is_empty() || parent == "/" {
        format!("/{name}")
    } else {
        format!("{parent}/{name}")
    }
}

fn parse_directory(data: &[u8]) -> Result<Vec<FileEntrySet>> {
    let mut entries = Vec::new();
    let mut off = 0usize;
    while off + DIR_ENTRY_SIZE <= data.len() {
        let raw = RawDirEntry::parse(&data[off..off + DIR_ENTRY_SIZE])?;
        let et = raw.entry_type();
        if et.is_end() {
            break;
        }
        if !et.in_use() {
            off += DIR_ENTRY_SIZE;
            continue;
        }
        if et.is_secondary() {
            return Err(Error::invalid(format!(
                "unexpected secondary directory entry {:#04x} at offset {off}",
                et.0
            )));
        }

        match et.0 {
            EntryType::ALLOC_BITMAP | EntryType::UPCASE_TABLE | EntryType::VOLUME_LABEL => {
                off += DIR_ENTRY_SIZE;
            }
            EntryType::FILE => {
                let file = FileDirEntry::from_raw(raw);
                let set_len = 1 + file.secondary_count as usize;
                let bytes = set_len * DIR_ENTRY_SIZE;
                if off + bytes > data.len() {
                    return Err(Error::invalid("truncated File directory entry set"));
                }
                entries.push(parse_file_set(&data[off..off + bytes], file)?);
                off += bytes;
            }
            EntryType::VOLUME_GUID => {
                let guid = VolumeGuidDirEntry::from_raw(raw);
                off = skip_set(off, data.len(), guid.secondary_count)?;
            }
            EntryType::TEXFAT_PADDING => {
                let primary = GenericPrimaryDirEntry::from_raw(raw);
                off = skip_set(off, data.len(), primary.secondary_count)?;
            }
            other if et.is_critical() => {
                return Err(Error::invalid(format!(
                    "unrecognized critical primary directory entry {other:#04x}"
                )));
            }
            _ => {
                let primary = GenericPrimaryDirEntry::from_raw(raw);
                off = skip_set(off, data.len(), primary.secondary_count)?;
            }
        }
    }
    Ok(entries)
}

fn parse_file_set(bytes: &[u8], file: FileDirEntry) -> Result<FileEntrySet> {
    let expected = (1 + file.secondary_count as usize) * DIR_ENTRY_SIZE;
    if bytes.len() < expected {
        return Err(Error::invalid("File directory entry set truncated"));
    }
    let stored = file.set_checksum;
    let computed = entry_set_checksum(&bytes[..expected]);
    if stored != computed {
        return Err(Error::invalid(format!(
            "File entry set checksum mismatch (stored {stored:#06x}, computed {computed:#06x})"
        )));
    }

    let mut stream = None;
    let mut names = Vec::new();
    let mut vendor_extensions = Vec::new();
    let mut vendor_allocations = Vec::new();

    let mut off = DIR_ENTRY_SIZE;
    while off + DIR_ENTRY_SIZE <= expected {
        let raw = RawDirEntry::parse(&bytes[off..off + DIR_ENTRY_SIZE])?;
        let et = raw.entry_type();
        if !et.in_use() || et.is_primary() {
            return Err(Error::invalid(format!(
                "invalid secondary in File set: {:#04x}",
                et.0
            )));
        }
        match et.0 {
            EntryType::STREAM_EXTENSION => {
                if stream.is_some() {
                    return Err(Error::invalid("multiple Stream Extension entries"));
                }
                stream = Some(StreamExtensionDirEntry::from_raw(raw));
            }
            EntryType::FILE_NAME => names.push(FileNameDirEntry::from_raw(raw)),
            EntryType::VENDOR_EXTENSION => {
                vendor_extensions.push(VendorExtensionDirEntry::from_raw(raw))
            }
            EntryType::VENDOR_ALLOCATION => {
                vendor_allocations.push(VendorAllocationDirEntry::from_raw(raw))
            }
            other if et.is_critical() => {
                return Err(Error::invalid(format!(
                    "unrecognized critical secondary {other:#04x}"
                )));
            }
            _ => {}
        }
        off += DIR_ENTRY_SIZE;
    }

    let stream = stream.ok_or_else(|| Error::invalid("File set missing Stream Extension"))?;
    if names.is_empty() {
        return Err(Error::invalid("File set missing File Name entry"));
    }
    let name = decode_file_name(&names, stream.name_length);
    Ok(FileEntrySet {
        file,
        stream,
        name,
        vendor_extensions,
        vendor_allocations,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("myexfat")
    }

    #[test]
    fn boot_sector_of_myexfat() {
        let vol = Volume::open(fixture()).unwrap();
        assert_eq!(&vol.boot.file_system_name, b"EXFAT   ");
        assert_eq!(vol.bytes_per_sector(), 512);
        assert_eq!(vol.cluster_size(), 16 * 1024);
        assert_eq!(vol.cluster_count(), 6144);
        assert_eq!(vol.boot.first_cluster_of_root_directory, 4);
        assert_eq!(vol.boot.number_of_fats, 1);
    }

    #[test]
    fn reports_fragmented_directories_on_myexfat() {
        let vol = Volume::open(fixture()).unwrap();
        let found = analyze(&vol).unwrap();
        let paths: Vec<&str> = found.iter().map(|e| e.path.as_str()).collect();
        assert!(
            paths.contains(&"/test") && paths.contains(&"/test/0") && paths.contains(&"/test/1"),
            "expected /test and children, got {paths:?}"
        );
        for e in &found {
            assert!(e.is_directory);
            assert!(e.chunks > 1);
            assert_eq!(e.size, 98304);
            assert_eq!(e.chunks, 6);
        }
    }
}
