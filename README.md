# rs-exfat-defrag

Offline reader for exFAT volumes (disk images or block devices). Right now it only **analyzes** fragmentation: it walks the directory tree and prints files and directories whose clusters are not a single contiguous run. Defragmentation is the intended next step, with directories as the main target.

On-disk layouts follow Microsoft’s [exFAT File System Specification](https://learn.microsoft.com/en-us/windows/win32/fileio/exfat-specification) revision 1.00.

## Usage

```bash
cargo run -- <filesystem>
```

`<filesystem>` is a path to an exFAT image or an unmounted block device.

```
TYPE         SIZE CHUNKS  CHUNK SIZES                             PATH
dir         98304      6  16K+16K+16K+16K+16K+16K                 /test
dir         98304      6  16K+16K+16K+16K+16K+16K                 /test/0
file        5000       3  2K+1K+904                               /test/1/note.txt
```

| Column        | Meaning |
|---------------|---------|
| `TYPE`        | `dir` or `file` |
| `SIZE`        | stream `DataLength` in bytes |
| `CHUNKS`      | number of contiguous cluster runs |
| `CHUNK SIZES` | byte size of each run, in chain order. Powers of two use binary units (`16K`, `1M`); other values stay a byte count. The last run may be a partial cluster. |
| `PATH`        | path from the volume root |

Only entries with **more than one chunk** are printed. A fully contiguous allocation is one chunk and is omitted.

The included `myexfat` image is a small sample volume:

```bash
cargo run -- myexfat
```

## Architecture

The crate is a library plus a thin CLI.

```
src/main.rs      CLI: open path, analyze, print the table
src/lib.rs       public API and error type
src/structs.rs   on-disk structures (boot, FAT, every directory entry)
src/volume.rs    open a volume, load the FAT, read cluster chains
src/analysis.rs  walk the tree, count runs, collect fragmented entries
```

**`structs`** — little-endian parsers for the boot sector, OEM/flash parameters, FAT constants, and directory entries (allocation bitmap, up-case table, volume label/GUID, file, stream extension, file name, vendor). Also boot-region and entry-set checksums.

**`volume`** — `Volume::open` checks the `EXFAT` signature and the 11-sector boot checksum (falls back to the backup boot region), then loads the active FAT. `cluster_chain` honors `NoFatChain` (contiguous run from `FirstCluster`) versus a FAT walk until `0xFFFFFFFF`. The root directory is always chained through the FAT, as the spec requires.

**`analysis`** — iterative directory walk. For each file/directory it resolves the cluster chain and counts maximal runs of consecutive cluster numbers. Entries with `chunks > 1` become a `FragmentedEntry`.
