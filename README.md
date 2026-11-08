# rs-exfat-defrag

Offline reader and defragmenter for exFAT volumes (disk images or unmounted block devices). It walks the directory tree, reports files and directories whose clusters are not a single contiguous run, and can rewrite those allocations.

On-disk layouts follow Microsoft’s [exFAT File System Specification](https://learn.microsoft.com/en-us/windows/win32/fileio/exfat-specification) revision 1.00.

The volume must not be mounted while `--defrag` is running.

## Usage

```bash
cargo run -- [options] <filesystem>
```

`<filesystem>` is a path to an exFAT image or an unmounted block device.

### Analysis (default)

```bash
cargo run -- myexfat
```

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
| `CHUNK SIZES` | byte size of each run, in chain order. Powers of two use binary units (`16K`, `1M`); other values stay a byte count. |
| `PATH`        | path from the volume root |

Only entries with **more than one chunk** are printed.

### Defragmentation

```bash
cargo run -- --defrag myexfat
cargo run -- --defrag --dir myexfat
cargo run -- --defrag --chunk 1M myexfat
cargo run -- --defrag --dir --dirfirst myexfat
```

| Flag          | Meaning |
|---------------|---------|
| `--defrag`    | rewrite cluster allocations |
| `--chunk SIZE`| keep extents of at least `SIZE` (e.g. `1M`, `16K`). Smaller fragments are merged; larger ones may stay. Without `--chunk`, each object is packed into a single run. |
| `--dir`       | only process directories (analysis and defrag) |
| `--dirfirst`  | pack directories at the start of the cluster heap (requires `--defrag`) |

Ctrl+C prints `stopping work` and exits after the current file or directory is fully relocated.

## Architecture

```
src/main.rs      CLI flags, size formatting, Ctrl+C
src/lib.rs       public API and error type
src/structs.rs   on-disk structures (boot, FAT, every directory entry)
src/volume.rs    open a volume, FAT, cluster I/O, boot checksums
src/analysis.rs  tree walk, fragmentation report
src/defrag.rs    relocate allocations, allocation bitmap, --dirfirst packing
```

**`structs`** — little-endian parsers for the boot sector, OEM/flash parameters, FAT constants, and directory entries. Also boot-region and entry-set checksums.

**`volume`** — `Volume::open` is read-only; `open_read_write` is used for `--defrag`. Boot checksum is verified (backup boot region is a fallback). Cluster chains honor `NoFatChain` versus a FAT walk until `0xFFFFFFFF`. The root directory is always chained through the FAT.

**`analysis`** — iterative directory walk. Each file/directory’s cluster chain is split into maximal consecutive runs. Entries with more than one run are reported.

**`defrag`** — for each object that needs work, finds a free contiguous run (or keeps extents ≥ `--chunk`), copies clusters, then updates the FAT, allocation bitmap, and the Stream Extension (`FirstCluster`, checksum). `--dirfirst` evacuates non-directories from the low cluster prefix, then packs directories there in tree order. The volume dirty flag is set for the duration of the pass.
