use std::fs::{File, OpenOptions};
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};

use crate::structs::*;
use crate::{Error, Result};

/// An opened exFAT volume (image file or block device).
pub struct Volume {
    file: File,
    path: PathBuf,
    pub boot: BootSector,
    pub oem: OemParameters,
    fat: Vec<u32>,
    fat_off: u64,
    writable: bool,
}

impl Volume {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with(path, false)
    }

    pub fn open_read_write(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with(path, true)
    }

    fn open_with(path: impl AsRef<Path>, writable: bool) -> Result<Self> {
        let path = path.as_ref();
        let file = OpenOptions::new().read(true).write(writable).open(path)?;
        let (boot, oem) = read_boot_region(&file)?;
        boot.validate()?;

        let bps = boot.bytes_per_sector();
        let active = boot.volume_flags.active_fat() as u32;
        let fat_sector = boot.fat_offset + active * boot.fat_length;
        let fat_off = fat_sector as u64 * bps as u64;
        let fat_entries = boot.cluster_count as usize + 2;
        let fat_bytes = fat_entries * 4;
        let fat_region_bytes = boot.fat_length as u64 * bps as u64;
        if fat_bytes as u64 > fat_region_bytes {
            return Err(Error::invalid(
                "FAT region smaller than ClusterCount requires",
            ));
        }

        let mut fat_buf = vec![0u8; fat_bytes];
        read_exact_at(&file, &mut fat_buf, fat_off)?;
        let mut fat = Vec::with_capacity(fat_entries);
        for chunk in fat_buf.as_chunks::<4>().0 {
            fat.push(u32::from_le_bytes(*chunk));
        }

        Ok(Self {
            file,
            path: path.to_path_buf(),
            boot,
            oem,
            fat,
            fat_off,
            writable,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn bytes_per_sector(&self) -> u32 {
        self.boot.bytes_per_sector()
    }

    pub fn cluster_size(&self) -> u32 {
        self.boot.cluster_size()
    }

    pub fn cluster_count(&self) -> u32 {
        self.boot.cluster_count
    }

    pub fn last_cluster(&self) -> u32 {
        self.boot.last_cluster()
    }

    pub fn fat_entry(&self, cluster: u32) -> Result<u32> {
        let idx = cluster as usize;
        self.fat
            .get(idx)
            .copied()
            .ok_or_else(|| Error::invalid(format!("FAT index {cluster} out of range")))
    }

    pub fn cluster_offset(&self, cluster: u32) -> Result<u64> {
        if cluster < 2 || cluster > self.last_cluster() {
            return Err(Error::invalid(format!("cluster {cluster} out of range")));
        }
        let heap = self.boot.cluster_heap_offset as u64 * self.bytes_per_sector() as u64;
        Ok(heap + (cluster as u64 - 2) * self.cluster_size() as u64)
    }

    pub fn read_cluster(&self, cluster: u32) -> Result<Vec<u8>> {
        let mut buf = vec![0u8; self.cluster_size() as usize];
        let off = self.cluster_offset(cluster)?;
        read_exact_at(&self.file, &mut buf, off)?;
        Ok(buf)
    }

    /// Clusters covering an allocation.
    ///
    /// If `no_fat_chain` is set the allocation is a contiguous run starting at
    /// `first_cluster`. Otherwise the active FAT is walked until `FAT_END`.
    pub fn cluster_chain(
        &self,
        first_cluster: u32,
        data_length: u64,
        no_fat_chain: bool,
    ) -> Result<Vec<u32>> {
        if first_cluster == 0 {
            if data_length != 0 {
                return Err(Error::invalid("FirstCluster is 0 but DataLength is not 0"));
            }
            return Ok(Vec::new());
        }
        if first_cluster < 2 || first_cluster > self.last_cluster() {
            return Err(Error::invalid(format!(
                "FirstCluster {first_cluster} out of range"
            )));
        }

        let cluster_size = self.cluster_size() as u64;
        let needed = if data_length == 0 {
            0
        } else {
            data_length.div_ceil(cluster_size)
        };

        if no_fat_chain {
            if needed == 0 {
                return Ok(Vec::new());
            }
            let mut chain = Vec::with_capacity(needed as usize);
            for i in 0..needed {
                let c = first_cluster as u64 + i;
                if c > self.last_cluster() as u64 {
                    return Err(Error::invalid("NoFatChain run exceeds cluster heap"));
                }
                chain.push(c as u32);
            }
            return Ok(chain);
        }

        let mut chain = Vec::new();
        let mut cluster = first_cluster;
        let max = self.cluster_count() as usize + 2;
        while chain.len() < max {
            if cluster < 2 || cluster > self.last_cluster() {
                return Err(Error::invalid(format!(
                    "cluster {cluster} in chain is out of range"
                )));
            }
            if chain.contains(&cluster) {
                return Err(Error::invalid(format!("FAT loop at cluster {cluster}")));
            }
            chain.push(cluster);
            let next = self.fat_entry(cluster)?;
            if next == FAT_END {
                break;
            }
            if next == FAT_BAD {
                return Err(Error::invalid(format!("bad cluster in chain at {cluster}")));
            }
            if next == FAT_FREE {
                return Err(Error::invalid(format!(
                    "free FAT entry in chain at {cluster}"
                )));
            }
            cluster = next;
        }
        if chain.last().is_some() && self.fat_entry(*chain.last().unwrap())? != FAT_END {
            return Err(Error::invalid("FAT chain exceeds ClusterCount without EOF"));
        }
        Ok(chain)
    }

    pub fn read_allocation(
        &self,
        first_cluster: u32,
        data_length: u64,
        no_fat_chain: bool,
    ) -> Result<Vec<u8>> {
        let chain = self.cluster_chain(first_cluster, data_length, no_fat_chain)?;
        if chain.is_empty() {
            return Ok(Vec::new());
        }
        let mut data = Vec::with_capacity(chain.len() * self.cluster_size() as usize);
        for cluster in chain {
            data.extend_from_slice(&self.read_cluster(cluster)?);
        }
        if data_length > 0 && data.len() as u64 > data_length {
            data.truncate(data_length as usize);
        }
        Ok(data)
    }

    /// Root directory contents. Size is the full cluster-chain allocation.
    pub fn read_root_directory(&self) -> Result<Vec<u8>> {
        let first = self.boot.first_cluster_of_root_directory;
        let chain = self.cluster_chain(first, 0, false)?;
        let data_length = chain.len() as u64 * self.cluster_size() as u64;
        self.read_allocation(first, data_length, false)
    }

    pub fn write_cluster(&self, cluster: u32, data: &[u8]) -> Result<()> {
        self.ensure_write()?;
        let size = self.cluster_size() as usize;
        if data.len() != size {
            return Err(Error::invalid(format!(
                "cluster write size {} != {size}",
                data.len()
            )));
        }
        let off = self.cluster_offset(cluster)?;
        write_all_at(&self.file, data, off)
    }

    pub fn set_fat(&mut self, cluster: u32, value: u32) -> Result<()> {
        self.ensure_write()?;
        let idx = cluster as usize;
        if idx >= self.fat.len() {
            return Err(Error::invalid(format!("FAT index {cluster} out of range")));
        }
        self.fat[idx] = value;
        write_all_at(
            &self.file,
            &value.to_le_bytes(),
            self.fat_off + (cluster as u64) * 4,
        )
    }

    pub fn read_in_chain(&self, chain: &[u32], offset: u64, buf: &mut [u8]) -> Result<()> {
        let cs = self.cluster_size() as u64;
        let mut done = 0usize;
        while done < buf.len() {
            let pos = offset + done as u64;
            let idx = (pos / cs) as usize;
            let inner = (pos % cs) as usize;
            if idx >= chain.len() {
                return Err(Error::invalid("read past end of cluster chain"));
            }
            let cluster = self.read_cluster(chain[idx])?;
            let n = (cluster.len() - inner).min(buf.len() - done);
            buf[done..done + n].copy_from_slice(&cluster[inner..inner + n]);
            done += n;
        }
        Ok(())
    }

    /// Write `data` at `offset` bytes into the allocation described by `chain`.
    pub fn write_in_chain(&self, chain: &[u32], offset: u64, data: &[u8]) -> Result<()> {
        self.ensure_write()?;
        let cs = self.cluster_size() as u64;
        let mut done = 0usize;
        while done < data.len() {
            let pos = offset + done as u64;
            let idx = (pos / cs) as usize;
            let inner = (pos % cs) as usize;
            if idx >= chain.len() {
                return Err(Error::invalid("write past end of cluster chain"));
            }
            let mut cluster = self.read_cluster(chain[idx])?;
            let n = (cluster.len() - inner).min(data.len() - done);
            cluster[inner..inner + n].copy_from_slice(&data[done..done + n]);
            self.write_cluster(chain[idx], &cluster)?;
            done += n;
        }
        Ok(())
    }

    pub fn set_volume_dirty(&mut self, dirty: bool) -> Result<()> {
        self.ensure_write()?;
        let mut flags = self.boot.volume_flags.0;
        if dirty {
            flags |= VolumeFlags::VOLUME_DIRTY;
        } else {
            flags &= !VolumeFlags::VOLUME_DIRTY;
        }
        self.boot.volume_flags = VolumeFlags(flags);
        write_all_at(&self.file, &flags.to_le_bytes(), 106)
    }

    pub fn write_root_first_cluster(&mut self, cluster: u32) -> Result<()> {
        self.ensure_write()?;
        if cluster < 2 || cluster > self.last_cluster() {
            return Err(Error::invalid(format!(
                "root cluster {cluster} out of range"
            )));
        }
        self.boot.first_cluster_of_root_directory = cluster;
        let bps = self.bytes_per_sector();
        self.patch_boot_root(0, bps, cluster)?;
        self.patch_boot_root(BACKUP_BOOT_SECTOR as u64 * bps as u64, bps, cluster)?;
        Ok(())
    }

    fn patch_boot_root(&self, region_off: u64, bps: u32, cluster: u32) -> Result<()> {
        let region_len = bps as usize * BOOT_REGION_SECTORS as usize;
        let mut region = vec![0u8; region_len];
        read_exact_at(&self.file, &mut region, region_off)?;
        region[96..100].copy_from_slice(&cluster.to_le_bytes());
        let checksum = boot_checksum(&region, bps);
        write_all_at(&self.file, &region, region_off)?;
        let mut cs_sector = vec![0u8; bps as usize];
        for chunk in cs_sector.as_chunks_mut::<4>().0 {
            chunk.copy_from_slice(&checksum.to_le_bytes());
        }
        write_all_at(
            &self.file,
            &cs_sector,
            region_off + BOOT_REGION_SECTORS as u64 * bps as u64,
        )
    }

    pub fn sync(&self) -> Result<()> {
        self.file.sync_data().map_err(Error::from)
    }

    fn ensure_write(&self) -> Result<()> {
        if !self.writable {
            return Err(Error::invalid("volume opened read-only"));
        }
        Ok(())
    }
}

fn read_exact_at(file: &File, buf: &mut [u8], offset: u64) -> Result<()> {
    file.read_exact_at(buf, offset).map_err(Error::from)
}

fn write_all_at(file: &File, buf: &[u8], offset: u64) -> Result<()> {
    file.write_all_at(buf, offset).map_err(Error::from)
}

fn read_boot_region(file: &File) -> Result<(BootSector, OemParameters)> {
    // Probe sector size from the first 512 bytes, then read the full 11-sector region.
    let mut probe = [0u8; BOOT_SECTOR_SIZE];
    read_exact_at(file, &mut probe, 0)?;
    let boot = BootSector::parse(&probe)?;
    // Validate enough to know sector size; full validate happens after checksum.
    if &boot.file_system_name != FS_NAME {
        return Err(Error::invalid(format!(
            "not an exFAT volume (FileSystemName = {:?})",
            String::from_utf8_lossy(&boot.file_system_name)
        )));
    }
    if !(9..=12).contains(&boot.bytes_per_sector_shift) {
        return Err(Error::invalid("BytesPerSectorShift out of range"));
    }
    let bps = boot.bytes_per_sector();
    let region_len = bps as usize * BOOT_REGION_SECTORS as usize;

    let try_region = |offset: u64| -> Result<(BootSector, OemParameters, bool)> {
        let mut region = vec![0u8; region_len];
        read_exact_at(file, &mut region, offset)?;
        let boot = BootSector::parse(&region[..BOOT_SECTOR_SIZE])?;
        let checksum = boot_checksum(&region, bps);
        let mut stored = [0u8; 4];
        // Boot checksum sub-region is the 12th sector of the 11+checksum? Spec: 11 sectors
        // hashed, checksum sector is index 11 (12th sector of the boot region).
        read_exact_at(file, &mut stored, offset + 11 * bps as u64)?;
        let stored_cs = u32::from_le_bytes(stored);
        let ok = checksum == stored_cs;
        let oem_off = 9 * bps as usize;
        let oem = OemParameters::parse(&region[oem_off..oem_off + bps as usize])?;
        Ok((boot, oem, ok))
    };

    let (boot, oem, main_ok) = try_region(0)?;
    if main_ok {
        return Ok((boot, oem));
    }

    let backup_off = BACKUP_BOOT_SECTOR as u64 * bps as u64;
    match try_region(backup_off) {
        Ok((boot, oem, true)) => Ok((boot, oem)),
        _ => Err(Error::invalid(
            "main boot checksum mismatch (computed boot region is corrupt; backup also unusable)",
        )),
    }
}

/// Byte size of each contiguous cluster run.
///
/// Full clusters are `cluster_size` bytes. The last run is truncated so the
/// sizes sum to `data_length` when that is smaller than the allocation.
pub fn chunk_sizes(chain: &[u32], cluster_size: u64, data_length: u64) -> Vec<u64> {
    if chain.is_empty() || cluster_size == 0 {
        return Vec::new();
    }

    let mut sizes = Vec::new();
    let mut run = 1u64;
    for pair in chain.windows(2) {
        if pair[1] == pair[0] + 1 {
            run += 1;
        } else {
            sizes.push(run * cluster_size);
            run = 1;
        }
    }
    sizes.push(run * cluster_size);

    if data_length == 0 {
        return sizes;
    }

    let mut remaining = data_length;
    for size in &mut sizes {
        if remaining == 0 {
            *size = 0;
        } else if *size > remaining {
            *size = remaining;
            remaining = 0;
        } else {
            remaining -= *size;
        }
    }
    sizes.retain(|&s| s > 0);
    sizes
}

/// Split a cluster chain into maximal runs of consecutive cluster numbers.
pub fn cluster_extents(chain: &[u32]) -> Vec<Vec<u32>> {
    let mut out = Vec::new();
    let mut cur = Vec::new();
    for &c in chain {
        if let Some(&last) = cur.last() {
            if c == last + 1 {
                cur.push(c);
                continue;
            }
            out.push(std::mem::take(&mut cur));
        }
        cur.push(c);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_of_contiguous_and_scattered() {
        let cs = 1;
        assert!(chunk_sizes(&[], cs, 0).is_empty());
        assert_eq!(chunk_sizes(&[5], cs, 1).len(), 1);
        assert_eq!(chunk_sizes(&[5, 6, 7], cs, 3).len(), 1);
        assert_eq!(chunk_sizes(&[5, 176, 348], cs, 3).len(), 3);
        assert_eq!(chunk_sizes(&[5, 6, 10, 11, 12], cs, 5).len(), 2);
    }

    #[test]
    fn chunk_sizes_full_and_partial() {
        let cs = 4096;
        assert!(chunk_sizes(&[], cs, 0).is_empty());
        assert_eq!(chunk_sizes(&[5], cs, 4096), vec![4096]);
        assert_eq!(chunk_sizes(&[5, 6, 7], cs, 3 * cs), vec![3 * cs]);
        assert_eq!(chunk_sizes(&[5, 176, 348], cs, 3 * cs), vec![cs, cs, cs]);
        assert_eq!(
            chunk_sizes(&[5, 6, 10, 11, 12], cs, 5 * cs),
            vec![2 * cs, 3 * cs]
        );
        assert_eq!(chunk_sizes(&[5, 7, 8], cs, 5000), vec![4096, 904]);
    }
}
