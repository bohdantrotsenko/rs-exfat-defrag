use crate::structs::*;
use crate::volume::{Volume, chunk_sizes};
use crate::{Error, Result};

/// A file or directory whose cluster allocation is not a single contiguous run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FragmentedEntry {
    pub path: String,
    pub is_directory: bool,
    pub size: u64,
    /// Byte size of each contiguous cluster run, in chain order.
    pub chunk_sizes: Vec<u64>,
    pub clusters: usize,
}

impl FragmentedEntry {
    pub fn chunks(&self) -> usize {
        self.chunk_sizes.len()
    }
}

/// A file or directory on the volume, with enough info to relocate it.
#[derive(Clone, Debug)]
pub struct FsEntry {
    pub path: String,
    pub is_directory: bool,
    pub is_root: bool,
    pub first_cluster: u32,
    pub data_length: u64,
    pub no_fat_chain: bool,
    pub chain: Vec<u32>,
    /// Parent path (`None` for root).
    pub parent: Option<String>,
    /// Byte offset of the File directory entry set in the parent directory.
    pub dirent_offset: u64,
    pub set_bytes: usize,
}

/// Result of a full tree walk, including allocation-bitmap / up-case locations.
#[derive(Clone, Debug)]
pub struct VolumeScan {
    pub entries: Vec<FsEntry>,
    pub bitmap: Option<AllocationBitmapDirEntry>,
    pub upcase: Option<UpcaseTableDirEntry>,
}

/// Walk the directory tree and collect files/directories with more than one cluster run.
pub fn analyze(vol: &Volume) -> Result<Vec<FragmentedEntry>> {
    analyze_filtered(vol, false)
}

pub fn analyze_filtered(vol: &Volume, dir_only: bool) -> Result<Vec<FragmentedEntry>> {
    let scan = walk_volume(vol)?;
    let cluster_size = vol.cluster_size() as u64;
    let mut out = Vec::new();
    for e in &scan.entries {
        if dir_only && !e.is_directory {
            continue;
        }
        let sizes = chunk_sizes(&e.chain, cluster_size, e.data_length);
        if sizes.len() > 1 {
            out.push(FragmentedEntry {
                path: e.path.clone(),
                is_directory: e.is_directory,
                size: e.data_length,
                chunk_sizes: sizes,
                clusters: e.chain.len(),
            });
        }
    }
    Ok(out)
}

pub fn walk_volume(vol: &Volume) -> Result<VolumeScan> {
    let mut entries = Vec::new();
    let mut bitmap = None;
    let mut upcase = None;

    let root_first = vol.boot.first_cluster_of_root_directory;
    let root_chain = vol.cluster_chain(root_first, 0, false)?;
    let cluster_size = vol.cluster_size() as u64;
    let root_size = root_chain.len() as u64 * cluster_size;
    entries.push(FsEntry {
        path: "/".to_string(),
        is_directory: true,
        is_root: true,
        first_cluster: root_first,
        data_length: root_size,
        no_fat_chain: false,
        chain: root_chain,
        parent: None,
        dirent_offset: 0,
        set_bytes: 0,
    });

    let root_data = vol.read_root_directory()?;
    let root_scan = scan_directory(&root_data)?;
    bitmap = root_scan.bitmap.or(bitmap);
    upcase = root_scan.upcase.or(upcase);

    let mut stack: Vec<(String, Vec<u8>)> = vec![(String::new(), root_data)];

    while let Some((dir_path, data)) = stack.pop() {
        let scan = if dir_path.is_empty() {
            root_scan.clone()
        } else {
            scan_directory(&data)?
        };
        let parent_path = if dir_path.is_empty() {
            "/".to_string()
        } else {
            dir_path.clone()
        };
        for (offset, record) in scan.files {
            let chain = vol.cluster_chain(
                record.first_cluster(),
                record.data_length(),
                record.no_fat_chain(),
            )?;
            let path = join_path(&dir_path, &record.name);
            let is_directory = record.is_directory();
            let first = record.first_cluster();
            let data_length = record.data_length();
            let no_fat_chain = record.no_fat_chain();
            let set_bytes = (1 + record.file.secondary_count as usize) * DIR_ENTRY_SIZE;
            if is_directory && first != 0 {
                let child = vol.read_allocation(first, data_length, no_fat_chain)?;
                stack.push((path.clone(), child));
            }
            entries.push(FsEntry {
                path,
                is_directory,
                is_root: false,
                first_cluster: first,
                data_length,
                no_fat_chain,
                chain,
                parent: Some(parent_path.clone()),
                dirent_offset: offset as u64,
                set_bytes,
            });
        }
    }

    Ok(VolumeScan {
        entries,
        bitmap,
        upcase,
    })
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

#[derive(Clone, Debug, Default)]
struct DirScan {
    files: Vec<(usize, FileEntrySet)>,
    bitmap: Option<AllocationBitmapDirEntry>,
    upcase: Option<UpcaseTableDirEntry>,
}

fn scan_directory(data: &[u8]) -> Result<DirScan> {
    let mut scan = DirScan::default();
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
            EntryType::ALLOC_BITMAP => {
                scan.bitmap = Some(AllocationBitmapDirEntry::from_raw(raw));
                off += DIR_ENTRY_SIZE;
            }
            EntryType::UPCASE_TABLE => {
                scan.upcase = Some(UpcaseTableDirEntry::from_raw(raw));
                off += DIR_ENTRY_SIZE;
            }
            EntryType::VOLUME_LABEL => {
                off += DIR_ENTRY_SIZE;
            }
            EntryType::FILE => {
                let file = FileDirEntry::from_raw(raw);
                let set_len = 1 + file.secondary_count as usize;
                let bytes = set_len * DIR_ENTRY_SIZE;
                if off + bytes > data.len() {
                    return Err(Error::invalid("truncated File directory entry set"));
                }
                let parsed = parse_file_set(&data[off..off + bytes], file)?;
                scan.files.push((off, parsed));
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
    Ok(scan)
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
            assert!(e.chunks() > 1);
            assert_eq!(e.size, 98304);
            assert_eq!(e.chunks(), 6);
            assert_eq!(e.chunk_sizes, vec![16384; 6]);
        }
    }
}
