use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::analysis::{FsEntry, walk_volume};
use crate::structs::*;
use crate::volume::{Volume, cluster_extents};
use crate::{Error, Result};

pub struct DefragOptions {
    /// Merge extents smaller than this many bytes. `None` means make each
    /// object a single contiguous run.
    pub min_chunk: Option<u64>,
    pub dir_only: bool,
    pub dir_first: bool,
}

pub struct DefragStats {
    pub moved: usize,
    pub skipped: usize,
    pub stopped: bool,
}

struct Bitmap {
    data: Vec<u8>,
    chain: Vec<u32>,
}

impl Bitmap {
    fn bit(cluster: u32) -> Result<(usize, u8)> {
        if cluster < 2 {
            return Err(Error::invalid(format!("bitmap cluster {cluster} < 2")));
        }
        let i = (cluster - 2) as usize;
        Ok((i / 8, (i % 8) as u8))
    }

    fn is_free(&self, cluster: u32) -> bool {
        match Self::bit(cluster) {
            Ok((byte, bit)) => self
                .data
                .get(byte)
                .map(|b| b & (1 << bit) == 0)
                .unwrap_or(false),
            Err(_) => false,
        }
    }

    fn set(&mut self, cluster: u32, allocated: bool) -> Result<()> {
        let (byte, bit) = Self::bit(cluster)?;
        if byte >= self.data.len() {
            return Err(Error::invalid("cluster beyond allocation bitmap"));
        }
        if allocated {
            self.data[byte] |= 1 << bit;
        } else {
            self.data[byte] &= !(1 << bit);
        }
        Ok(())
    }

    fn flush(&self, vol: &Volume) -> Result<()> {
        vol.write_in_chain(&self.chain, 0, &self.data)
    }
}

struct Session<'a> {
    vol: &'a mut Volume,
    bitmap: Bitmap,
    entries: HashMap<String, FsEntry>,
    owner: HashMap<u32, String>,
    system: HashSet<u32>,
    stop: &'a AtomicBool,
    stats: DefragStats,
}

pub fn defrag(vol: &mut Volume, opts: &DefragOptions, stop: &AtomicBool) -> Result<DefragStats> {
    let scan = walk_volume(vol)?;
    let bitmap_ent = scan
        .bitmap
        .ok_or_else(|| Error::invalid("volume has no Allocation Bitmap directory entry"))?;
    let upcase = scan.upcase;

    let bitmap_chain =
        vol.cluster_chain(bitmap_ent.first_cluster, bitmap_ent.data_length, false)?;
    let mut bitmap_data =
        vol.read_allocation(bitmap_ent.first_cluster, bitmap_ent.data_length, false)?;
    bitmap_data.truncate(bitmap_ent.data_length as usize);

    let mut system = HashSet::new();
    for c in &bitmap_chain {
        system.insert(*c);
    }
    if let Some(up) = upcase {
        for c in vol.cluster_chain(up.first_cluster, up.data_length, false)? {
            system.insert(c);
        }
    }

    let mut entries = HashMap::new();
    let mut owner = HashMap::new();
    for e in scan.entries {
        for c in &e.chain {
            owner.insert(*c, e.path.clone());
        }
        entries.insert(e.path.clone(), e);
    }

    let mut sess = Session {
        bitmap: Bitmap {
            data: bitmap_data,
            chain: bitmap_chain,
        },
        vol,
        entries,
        owner,
        system,
        stop,
        stats: DefragStats {
            moved: 0,
            skipped: 0,
            stopped: false,
        },
    };

    sess.vol.set_volume_dirty(true)?;

    let result = sess.run(opts);
    if result.is_ok() {
        sess.vol.set_volume_dirty(false)?;
        sess.vol.sync()?;
    } else {
        let _ = sess.vol.sync();
    }
    result?;
    Ok(sess.stats)
}

impl Session<'_> {
    fn run(&mut self, opts: &DefragOptions) -> Result<()> {
        if opts.dir_first {
            self.pack_directories_first()?;
            if self.stop.load(Ordering::SeqCst) {
                self.stats.stopped = true;
                return Ok(());
            }
        }

        let mut paths: Vec<String> = self.entries.keys().cloned().collect();
        paths.sort();
        for path in paths {
            if self.stop.load(Ordering::SeqCst) {
                self.stats.stopped = true;
                return Ok(());
            }
            let e = self.entries[&path].clone();
            if opts.dir_only && !e.is_directory {
                continue;
            }
            if e.chain.is_empty() {
                self.stats.skipped += 1;
                continue;
            }
            let dest = match self.plan_chunk_dest(&e, opts.min_chunk)? {
                None => {
                    self.stats.skipped += 1;
                    continue;
                }
                Some(d) => d,
            };
            println!("defrag {} {}", kind_of(&e), e.path);
            self.relocate(&path, &dest)?;
        }
        Ok(())
    }

    fn pack_directories_first(&mut self) -> Result<()> {
        let mut dirs: Vec<String> = self
            .entries
            .values()
            .filter(|e| e.is_directory && !e.chain.is_empty())
            .map(|e| e.path.clone())
            .collect();
        dirs.sort_by(|a, b| {
            let da = dir_sort_key(a);
            let db = dir_sort_key(b);
            da.cmp(&db)
        });

        let total: usize = dirs.iter().map(|p| self.entries[p].chain.len()).sum();
        let prefix = self.prefix_clusters(total)?;

        // Evacuate files that sit in the directory prefix.
        let files: Vec<String> = self
            .entries
            .values()
            .filter(|e| !e.is_directory && !e.chain.is_empty())
            .map(|e| e.path.clone())
            .collect();
        let prefix_set: HashSet<u32> = prefix.iter().copied().collect();
        for path in files {
            if self.stop.load(Ordering::SeqCst) {
                self.stats.stopped = true;
                return Ok(());
            }
            let e = self.entries[&path].clone();
            if !e.chain.iter().any(|c| prefix_set.contains(c)) {
                continue;
            }
            let n = e.chain.len() as u32;
            let start = self
                .find_free_run(n, &e.chain.iter().copied().collect(), Some(&prefix_set), 2)?
                .ok_or_else(|| {
                    Error::invalid(format!(
                        "not enough free space to evacuate {} from the directory prefix",
                        e.path
                    ))
                })?;
            let dest: Vec<u32> = (start..start + n).collect();
            self.relocate(&path, &dest)?;
        }

        let mut cursor = 0usize;
        for path in dirs {
            if self.stop.load(Ordering::SeqCst) {
                self.stats.stopped = true;
                return Ok(());
            }
            let e = self.entries[&path].clone();
            let n = e.chain.len();
            let dest = prefix[cursor..cursor + n].to_vec();
            cursor += n;
            if e.chain == dest {
                self.stats.skipped += 1;
                continue;
            }
            self.ensure_dest_available(&dest, &e.path)?;
            if self.stats.stopped {
                return Ok(());
            }
            println!("dirfirst {} -> cluster {}", e.path, dest[0]);
            self.relocate(&path, &dest)?;
        }
        Ok(())
    }

    fn prefix_clusters(&self, count: usize) -> Result<Vec<u32>> {
        let mut out = Vec::with_capacity(count);
        let last = self.vol.last_cluster();
        for c in 2..=last {
            if self.system.contains(&c) {
                continue;
            }
            if self.vol.fat_entry(c)? == FAT_BAD {
                continue;
            }
            out.push(c);
            if out.len() == count {
                return Ok(out);
            }
        }
        Err(Error::invalid(
            "not enough clusters after the system area to pack directories",
        ))
    }

    fn ensure_dest_available(&mut self, dest: &[u32], self_path: &str) -> Result<()> {
        let self_clusters: HashSet<u32> = self.entries[self_path].chain.iter().copied().collect();
        let mut blockers: HashSet<String> = HashSet::new();
        for c in dest {
            if self_clusters.contains(c) {
                continue;
            }
            if self.bitmap.is_free(*c) {
                continue;
            }
            if let Some(owner) = self.owner.get(c)
                && owner != self_path
            {
                blockers.insert(owner.clone());
            }
        }
        for blocker in blockers {
            if self.stop.load(Ordering::SeqCst) {
                self.stats.stopped = true;
                return Ok(());
            }
            let e = self.entries[&blocker].clone();
            let n = e.chain.len() as u32;
            if n == 0 {
                continue;
            }
            let exclude: HashSet<u32> = dest
                .iter()
                .copied()
                .chain(e.chain.iter().copied())
                .collect();
            let start = self.find_free_run(n, &exclude, None, 2)?.ok_or_else(|| {
                Error::invalid(format!(
                    "not enough free space to move blocking object {}",
                    e.path
                ))
            })?;
            let tmp: Vec<u32> = (start..start + n).collect();
            self.relocate(&blocker, &tmp)?;
        }
        Ok(())
    }

    fn plan_chunk_dest(&self, e: &FsEntry, min_chunk: Option<u64>) -> Result<Option<Vec<u32>>> {
        if e.chain.is_empty() {
            return Ok(None);
        }
        let cluster_size = self.vol.cluster_size() as u64;
        let min_clusters = match min_chunk {
            None => u32::MAX,
            Some(bytes) => bytes.div_ceil(cluster_size).max(1) as u32,
        };

        let extents = cluster_extents(&e.chain);
        let mut new_chain = Vec::with_capacity(e.chain.len());
        let mut small: Vec<u32> = Vec::new();
        let mut reserved: HashSet<u32> = HashSet::new();
        let mut source_exclude: HashSet<u32> = e.chain.iter().copied().collect();

        for ext in extents {
            let keep = min_clusters != u32::MAX && (ext.len() as u32) >= min_clusters;
            if keep {
                self.flush_small_group(
                    e,
                    &mut small,
                    &mut new_chain,
                    &mut reserved,
                    &mut source_exclude,
                )?;
                new_chain.extend_from_slice(&ext);
            } else {
                small.extend(ext);
            }
        }
        self.flush_small_group(
            e,
            &mut small,
            &mut new_chain,
            &mut reserved,
            &mut source_exclude,
        )?;

        if new_chain == e.chain {
            return Ok(None);
        }
        if new_chain.len() != e.chain.len() {
            return Err(Error::invalid(format!(
                "internal error: dest length {} != source {} for {}",
                new_chain.len(),
                e.chain.len(),
                e.path
            )));
        }
        Ok(Some(new_chain))
    }

    fn flush_small_group(
        &self,
        e: &FsEntry,
        small: &mut Vec<u32>,
        out: &mut Vec<u32>,
        reserved: &mut HashSet<u32>,
        source_exclude: &mut HashSet<u32>,
    ) -> Result<()> {
        if small.is_empty() {
            return Ok(());
        }
        if is_contiguous(small) {
            out.extend_from_slice(small);
            small.clear();
            return Ok(());
        }
        let n = small.len() as u32;
        let start = self
            .find_free_run(n, source_exclude, Some(reserved), 2)?
            .ok_or_else(|| {
                Error::invalid(format!(
                    "not enough contiguous free space to defrag {} ({n} clusters)",
                    e.path
                ))
            })?;
        for c in start..start + n {
            reserved.insert(c);
            source_exclude.insert(c);
            out.push(c);
        }
        small.clear();
        Ok(())
    }

    fn find_free_run(
        &self,
        count: u32,
        exclude: &HashSet<u32>,
        also_exclude: Option<&HashSet<u32>>,
        from: u32,
    ) -> Result<Option<u32>> {
        if count == 0 {
            return Ok(None);
        }
        let last = self.vol.last_cluster();
        let mut run = 0u32;
        let mut start = 0u32;
        for c in from.max(2)..=last {
            let blocked = self.system.contains(&c)
                || exclude.contains(&c)
                || also_exclude.map(|s| s.contains(&c)).unwrap_or(false)
                || !self.bitmap.is_free(c)
                || self.vol.fat_entry(c)? == FAT_BAD;
            if blocked {
                run = 0;
                continue;
            }
            if run == 0 {
                start = c;
            }
            run += 1;
            if run == count {
                return Ok(Some(start));
            }
        }
        Ok(None)
    }

    fn relocate(&mut self, path: &str, dest: &[u32]) -> Result<()> {
        let e = self.entries[path].clone();
        if dest.len() != e.chain.len() {
            return Err(Error::invalid("relocate dest length mismatch"));
        }
        if dest.is_empty() {
            return Ok(());
        }
        for c in dest {
            if self.system.contains(c) {
                return Err(Error::invalid(format!(
                    "refusing to overwrite system cluster {c}"
                )));
            }
        }

        let src = e.chain.clone();
        let src_set: HashSet<u32> = src.iter().copied().collect();
        let dest_set: HashSet<u32> = dest.iter().copied().collect();
        let overlap = src_set.intersection(&dest_set).next().is_some();

        if overlap || e.is_directory {
            let mut data = Vec::new();
            for c in &src {
                data.extend_from_slice(&self.vol.read_cluster(*c)?);
            }
            let cs = self.vol.cluster_size() as usize;
            for (i, d) in dest.iter().enumerate() {
                self.vol.write_cluster(*d, &data[i * cs..(i + 1) * cs])?;
            }
        } else {
            for (s, d) in src.iter().zip(dest.iter()) {
                if s != d {
                    let buf = self.vol.read_cluster(*s)?;
                    self.vol.write_cluster(*d, &buf)?;
                }
            }
        }

        for w in dest.windows(2) {
            self.vol.set_fat(w[0], w[1])?;
        }
        self.vol.set_fat(*dest.last().unwrap(), FAT_END)?;

        for c in dest {
            self.bitmap.set(*c, true)?;
        }
        for c in &src {
            if !dest_set.contains(c) {
                self.vol.set_fat(*c, FAT_FREE)?;
                self.bitmap.set(*c, false)?;
            }
        }
        self.bitmap.flush(self.vol)?;

        if e.is_root {
            self.vol.write_root_first_cluster(dest[0])?;
        } else {
            self.update_dirent(&e, dest[0])?;
        }
        self.vol.sync()?;

        for c in &src {
            if self.owner.get(c).map(String::as_str) == Some(path) {
                self.owner.remove(c);
            }
        }
        for c in dest {
            self.owner.insert(*c, path.to_string());
        }

        let live = self.entries.get_mut(path).unwrap();
        live.first_cluster = dest[0];
        live.chain = dest.to_vec();
        live.no_fat_chain = false;

        self.stats.moved += 1;
        Ok(())
    }

    fn update_dirent(&self, e: &FsEntry, first_cluster: u32) -> Result<()> {
        let parent_path = e
            .parent
            .as_ref()
            .ok_or_else(|| Error::invalid("non-root entry has no parent"))?;
        let parent = self
            .entries
            .get(parent_path)
            .ok_or_else(|| Error::invalid(format!("missing parent {parent_path}")))?;
        let mut set = vec![0u8; e.set_bytes];
        self.vol
            .read_in_chain(&parent.chain, e.dirent_offset, &mut set)?;
        if set[0] != EntryType::FILE {
            return Err(Error::invalid(format!(
                "directory entry for {} is not a File entry",
                e.path
            )));
        }
        if set.len() < DIR_ENTRY_SIZE * 2 {
            return Err(Error::invalid("File entry set too small"));
        }
        let stream = DIR_ENTRY_SIZE;
        if set[stream] != EntryType::STREAM_EXTENSION {
            return Err(Error::invalid(format!(
                "missing Stream Extension for {}",
                e.path
            )));
        }
        set[stream + 1] &= !GeneralFlags::NO_FAT_CHAIN;
        set[stream + 1] |= GeneralFlags::ALLOCATION_POSSIBLE;
        set[stream + 20..stream + 24].copy_from_slice(&first_cluster.to_le_bytes());
        let checksum = entry_set_checksum(&set);
        set[2..4].copy_from_slice(&checksum.to_le_bytes());
        self.vol
            .write_in_chain(&parent.chain, e.dirent_offset, &set)?;
        Ok(())
    }
}

fn is_contiguous(chain: &[u32]) -> bool {
    chain.windows(2).all(|w| w[1] == w[0] + 1)
}

fn kind_of(e: &FsEntry) -> &'static str {
    if e.is_directory { "dir" } else { "file" }
}

fn dir_sort_key(path: &str) -> (usize, String) {
    let depth = if path == "/" {
        0
    } else {
        path.bytes().filter(|b| *b == b'/').count()
    };
    (depth, path.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyze;
    use std::path::PathBuf;
    use std::sync::atomic::AtomicBool;

    fn fixture() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("myexfat")
    }

    #[test]
    fn defrag_clears_fragmented_dirs() {
        let src = fixture();
        let tmp = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/test-defrag-myexfat");
        std::fs::create_dir_all(tmp.parent().unwrap()).unwrap();
        std::fs::copy(&src, &tmp).unwrap();

        let stop = AtomicBool::new(false);
        {
            let mut vol = Volume::open_read_write(&tmp).unwrap();
            let stats = defrag(
                &mut vol,
                &DefragOptions {
                    min_chunk: None,
                    dir_only: true,
                    dir_first: false,
                },
                &stop,
            )
            .unwrap();
            assert!(stats.moved >= 3, "moved {}", stats.moved);
            assert!(!stats.stopped);
        }

        let vol = Volume::open(&tmp).unwrap();
        let left = analyze(&vol).unwrap();
        assert!(
            left.is_empty(),
            "still fragmented: {:?}",
            left.iter().map(|e| e.path.as_str()).collect::<Vec<_>>()
        );
    }
}
