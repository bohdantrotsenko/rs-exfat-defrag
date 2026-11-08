{{Filesystems}}
{{In Progress}}

ExFAT, as its name suggests, is an extension of the traditional FAT filesystem. As opposed to older versions, such as FAT16, exFAT has been greatly modernized and has more conveniences from a programming standpoint.

After a long period of keeping the official exFAT spec under their collective hats, Microsoft has finally published it at https://github.com/MicrosoftDocs/win32/blob/docs/desktop-src/FileIO/exfat-specification.md on August 27th, 2019. The major structures of the file system are described below.

== Partition Layout ==

{| class="wikitable"
|+ Overview of the order of structures in an exFAT partition
! Section !! Contents !! Offset (sector number) !! Size (in sectors)
|-
| rowspan = 5 | Main Boot Region || [[#Boot_Sector_Structure|Boot Sector]] || 0 || 1
|-
| [[#Extended_Boot_Sectors_Structure|Extended Boot Sectors]] || 1 || 8
|-
| [[#OEM_Parameters_Structure|OEM Parameters]] || 9 || 1
|-
| Reserved sectors || 10 || 1
|-
| [[#Boot_Checksum_Calculation|Boot checksum]] || 11 || 1
|-
| rowspan = 5 | Backup Boot Region || Boot Sector || 12 || 1
|-
| Extended Boot Sectors || 13 || 8
|-
| OEM Parameters || 21 || 1
|-
| Reserved sectors || 22 || 1
|-
| Boot checksum || 23 || 1
|-
| rowspan = 3 | FAT Region || FAT Alignment || 24 || FATOffset - 24
|-
| [[#FAT_Region_Structure|First FAT]] || FATOffset || FATLength
|-
| Second FAT || FATOffset + FATLength || FATLength * (NumberOfFATs - 1)
|-
| rowspan = 3 | Data Region || Cluster Heap Alignment || FATOffset + FATLength * NumberOfFATs || ClusterHeapOffset - (FATOffset + FATLength * NumberOfFATs)
|-
| Cluster Heap || ClusterheapOffset || ClusterCount * 2<sup>SectorsPerClusterShift</sup>
|-
| Excess Space (to end of partition or disk) || ClusterheapOffset + ClusterCount * 2<sup>SectorsPerClusterShift</sup> || VolumeLength - (ClusterHeapOffset + ClusterCount * 2<sup>SectorsPerClusterShift</sup>)
|-
|}

The Backup Boot Region is a duplicate of the Main Boot Region.

The FATAlignment and Cluster Heap Alignment sections appear to be filler to ensure that the FAT and Cluster Heap both start on a good boundary. This has more meaning for removeable media such as flash drives than for hard drives.


== Structures ==

=== Boot Sector Structure ===

{| class="wikitable"
|-
! Offset !! Size !! Name !! Description
|-
| 0x000 || 3 || JumpBoot || Jump instruction to start of boot code. For x86 CPUs, the byte sequence is usually EB 76 90.
|-
| 0x003 || 8 || FileSystemName || The "EXFAT&nbsp;&nbsp;&nbsp;" signature.
|-
| 0x00B || 53 || MustBeZero || This range of bytes is set null to help existing FAT drivers from mistakenly parsing an exFAT partition under the assumption that it contains valid FAT16 or FAT32 data.
|-
| 0x040 || 8 || PartitionOffset || The sector offset of the start of this partition relative to the start of the volume. Can be null.
|-
| 0x048 || 8 || VolumeLength || The total size of this exFAT partition in sectors. Should not be smaller than 1 MiB.
|-
| 0x050 || 4 || FATOffset || Offset in sectors from the start of the partition to the first FAT
|-
| 0x054 || 4 || FATLength || Size of each FAT in sectors
|-
| 0x058 || 4 || ClusterHeapOffset || Offset in sectors from the start of the partition to the start of the "Cluster Heap", known as the "Data Area" on former FAT versions.
|-
| 0x05C || 4 || ClusterCount || The total number of clusters in the Cluster Heap.
|-
| 0x060 || 4 || FirstClusterOfRootDirectory || The starting cluster of the root directory's cluster chain.
|-
| 0x064 || 4 || VolumeSerialNumber || A serial number for this partition, usually comprised by calculating the date and time of format into a single 64-bit value. There is no standard for this calculation; each FAT driver is free to use its own technique.
|-
| 0x068 || 2 || FileSystemRevision || Specifies the version of exFAT specification to which the structures in this partition adhere. For now, there is only a single exFAT specification; version 1.00. However in the future there may be more than this, and a driver should not attempt to parse exFAT structures of an exFAT specification version with which it is not familiar.
|-
| 0x06A || 2 || VolumeFlags || 
{| class="wikitable"
|-
! Bit !! Name !! Description
|-
| 0 || ActiveFAT || Used to implement transaction-safe mechanisms<br>
0 - First FAT and first allocation bitmap are valid<br>
1 - Second FAT and second allocation bitmap are valid
|-
| 1 || VolumeDirty || 0 - Partition is likely in a good state<br>
1 - Partition is likely not in a good state
|-
| 2 || MediaFailure || 0 - No bad clusters aside from those already marked "bad"<br>
1 - An I/O operation failed at some point indicating that the entire partition should be scanned for bad clusters at next mount.
|-
| 3 || ClearToZero || No real meaning... yet!
|-
| 4 - 15 || n/a || Reserved
|-
|}
This word is not included in checksumming.
|-
| 0x06C || 1 || BytesPerSectorShift || The number of places one would need to shift to translate bytes to sectors or sectors to bytes. Serves as an assist to allow shifting instead of multiplication or division.
|-
| 0x06D || 1 || SectorsPerClusterShift || Similar to the above, except used to calculate sectors per cluster.
|-
| 0x06E || 1 || NumberOfFATs || Useful when implementing transaction-safe mechanisms<br>
1 - Partition uses only a single FAT and Allocation Bitmaps<br>
2 - Partition uses two FATs and two Allocation Bitmaps
|-
| 0x06F || 1 || DriveSelect || Similar to the Drive Number field found at offset 0x24 in previous FAT versions. Usually 0x80 for hard disks.
|-
| 0x070 || 1 || PercentInUse || Percent of clusters in the Cluster Heap which are allocated, rounded down to the nearest whole number. This byte is not included in checksumming.
|-
| 0x071 || 7 || Reserved || Do not use
|-
| 0x078 || 390 || BootCode || Boot code pointed to by the initial jump instruction at the beginning of the structure
|-
| 0x1FE || 2 || BootSignature || A series of bytes (0x55, 0xAA) which indicates this is a proper boot sector, similar to previous FAT versions. Should be verified prior to depending on any other fields in the Boot Sector.
|-
|}

=== Extended Boot Sectors Structure ===

{| class="wikitable"
|-
! Offset !! Size !! Name !! Description
|-
| 0x00 || 2<sup>BytesPerSectorShift</sup> - 4 || ExtendedBootCode || Additional space for boot code.
|-
| 2<sup>BytesPerSectorShift</sup> - 4 || 4 || ExtendedBootSignature || A series of bytes (0x00, 0x00, 0x55, 0xAA) which indicates this is a proper boot sector, similar to previous FAT versions, but now a 32-bit value instead of 16-bit. Should be verified prior to depending on any other fields in the Extended Boot Sector. Note that this signature will appear on <i>each</i> of the Extended Boot Sectors.
|-
|}



=== OEM Parameters Structure ===

{| class="wikitable"
|-
! Offset !! Size !! Name !! Description
|-
| 0x000 || 48 || Parameters[0] || rowspan = 10 | A group of parameters in Generic Parameters format.

Each 48-byte entry may be one of:
{| class="wikitable"
|+ NullParameters
|-
! Offset !! Size !! Name !! Description
|-
| 0x00 || 16 || ParametersGUID || The "NullParameters" GUID:<br>
00000000-0000-0000-0000-000000000000
|-
| 0x10 || 32 || CustomDefined || Reserved, do not use
|-
|}

or

{| class="wikitable"
|+ FlashParameters
|-
! Offset !! Size !! Name !! Description
|-
| 0x00 || 16 || ParametersGUID || The "FlashParameters" GUID:<br>
0A0C7E46-3399-4021-90C8-FA6D389C4BA2
|-
| 0x10 || 4 || EraseBlockSize || The size of the flash media's erase block, in bytes
|-
| 0x14 || 4 || PageSize || The page size, in bytes, of the flash media
|-
| 0x18 || 4 || SpareSectors || The number of sectors the flash media has free for use in sparing operations
|-
| 0x1C || 4 || RandomAccessTime || The average number of nanoseconds the flash media needs to randomly access its memory
|-
| 0x20 || 4 || ProgrammingTime || The average number of nanoseconds the flash media needs for a programming operation
|-
| 0x24 || 4 || ReadCycle || The average number of nanoseconds the flash media needs to perform a read cycle
|-
| 0x28 || 4 || WriteCycle || The average number of nanoseconds the flash media needs to perform a write cycle
|-
| 0x2C || 4 || Reserved || Do not use
|-
|}
As of this writing, the exFAT specification only defines the two GUIDs above; additional ones may be defined in future versions.

|-
| 0x030 || 48 || Parameters[1]
|-
| 0x060 || 48 || Parameters[2]
|-
| 0x090 || 48 || Parameters[3]
|-
| 0x0C0 || 48 || Parameters[4]
|-
| 0x0F0 || 48 || Parameters[5]
|-
| 0x120 || 48 || Parameters[6]
|-
| 0x150 || 48 || Parameters[7]
|-
| 0x180 || 48 || Parameters[8]
|-
| 0x1B0 || 48 || Parameters[9]
|-
| 0x1E0 || 2<sup>BytesPerSectorShift</sup> - 480 || Reserved || Do not use
|-
|}



=== FAT Region Structure ===
{| class="wikitable"
|-
! Offset !! Size !! Name !! Description
|-
| 0x00 || 4 || FatEntry[0] || Comprised of a Media Descriptor byte (always 0xF8) followed by three bytes of 0xFF each 
|-
| 0x04 || 4 || FatEntry[1] || Always 0xFFFFFFFF. Do not use, interpret, or change.
|-
| 0x08 || 4 || FatEntry[2] || One of the following:

{| class="wikitable"
|-
! Value !! Description
|-
| 0x00000000 || Denotes a free cluster available for allocation
|-
| 0x00000002 through ClusterCount - 1 || A pointer to the next cluster in the chain
|-
| 0xFFFFFFF7 || Bad cluster
|-
| 0xFFFFFFFF || End-of-chain markeer
|-
|}
These are identical to the values used in previous FAT versions, with the difference that 0x00000001, 0xFFFFFFF0 - 0xFFFFFFF6, and 0xFFFFFFF8 - 0xFFFFFFFE are no longer ascribed any significance.

|-
| ... || ... || ... || ...
|-
|}




== C Code Examples ==

=== Filesystem Header ===

<syntaxhighlight lang="C">
struct bootsector {
	char jump[3];
	char fsid[8];
	char pad[53];
	uint64_t unk; // sectors before first cluster too?
	uint64_t fsSizeInSectors;
        uint32_t sectorsToStartOfFat;
        uint32_t sectorsUsedForFat;
        uint32_t sectorStartOfFirstCluster;
        uint32_t clusterCountInFs;
        uint32_t clusterForRootDirectory;
        uint32_t unk[2];
        uint8_t logBytesPerSector, logSectorsPerCluster, fatCount, driveId; // 2-log for both. bytesPerCluster = 1 << (logBytesPerSector + logSectorsPerCluster).
        uint8_t percentInUse; // for display ala windows computer explorer
        uint8_t unk[7];
        char bootcode[390];
        char bootsign[2];
};
</syntaxhighlight>
Block bitmap is cluster 2, upcase tbl is cluster 3. 


=== Boot Checksum Calculation ===
<syntaxhighlight lang="C">
UInt32 BootChecksum (UCHAR * Sectors, USHORT BytesPerSector)
{
  /*
    UCHAR * Sectors, above, points to an in-memory copy of the 11 sectors
    That is, 0 - 10 for the Main Boot Region, or 12 - 22 for the Backup Boot Region
  */

  UInt32 NumberOfBytes = (UInt32)BytesPerSector * 11;
  UInt32 Checksum = 0;
  UInt32 Index;

  for (Index = 0; Index < NumberOfBytes; Index++)
  {
    if ((Index == 106) || (Index == 107) || (Index == 112))
    {
      continue;
    }
    Checksum = ((Checksum&1) ? 0x80000000 : 0) + (Checksum>>1) + (UInt32)Sectors[Index];
  }
  return Checksum;
}
</syntaxhighlight>



=== Directory entries ===

32-byte entries. Entries contain one of a few possible entry types, first byte identifies which type it is. The second byte identifies how many subsequent entries are part of this entry. All bytes after are dependent on the type of entry. Any entry with the highest bit of entrytype set is not used.

<syntaxhighlight lang="c">
struct {
        uint8_t entrytype = 0x85;
        uint8_t entrycount; // This is the number of subsequent entries that also belong to this "file" entry. Should be at least 2, one 0xC0 and one 0xC1 for info and filename.
        uint16_t checksum;
        uint16_t flags; // 0x10 == directory
        uint16_t rsvd; // Reserved
        uint32_t creation, modification, access;
        char pad[12];
} fileEntry;
</syntaxhighlight>

<syntaxhighlight lang="c">
struct {
        uint8_t entryType = 0xC0;
        uint8_t flags;
        uint8_t reserved;
        uint8_t filenameLengthInBytes;
        uint16_t filenameHash;
        uint16_t reserved_2;
        uint64_t valid_filesize;
        uint32_t reserved3;
        uint32_t startCluster; // minus 2! Same as fat12/16/32.
        uint64_t filesize;
} fileInfoEntry;
</syntaxhighlight>

<syntaxhighlight lang="C">
struct {
        uint8_t entrytype = 0xC1, entrycount = 0x00;
        uint16_t name[15];
} filenameEntry;
</syntaxhighlight>



== Disk Hexdump Examples ==

=== Short filenames ===
    01040300  85 02 c8 f9 10 00 00 00  5b a9 47 45 6f a9 47 45  |........[.GEo.GE|
    01040310  6f a9 47 45 25 25 88 88  88 00 00 00 00 00 00 00  |o.GE%%..........|
    01040320  c0 03 00 05 ae 26 00 00  00 00 02 00 00 00 00 00  |.....&..........|
    01040330  00 00 00 00 17 00 00 00  00 00 02 00 00 00 00 00  |................|
    01040340  c1 00 69 00 6d 00 61 00  67 00 65 00 00 00 00 00  |..i.m.a.g.e.....|
    01040350  00 00 00 00 00 00 00 00  00 00 00 00 00 00 00 00  |................|

=== Longer filenames ===
    01080060  85 03 34 aa 10 00 00 00  1c a5 47 45 92 0a 48 45  |..4.......GE..HE|
    01080070  92 0a 48 45 94 94 88 88  88 00 00 00 00 00 00 00  |..HE............|
    01080080  c0 03 00 18 23 20 00 00  00 00 02 00 00 00 00 00  |....# ..........|
    01080090  00 00 00 00 07 00 00 00  00 00 02 00 00 00 00 00  |................|
    010800a0  c1 00 63 00 6f 00 6d 00  2e 00 67 00 6f 00 6f 00  |..c.o.m...g.o.o.|
    010800b0  67 00 6c 00 65 00 2e 00  61 00 6e 00 64 00 72 00  |g.l.e...a.n.d.r.|
    010800c0  c1 00 6f 00 69 00 64 00  2e 00 6d 00 75 00 73 00  |..o.i.d...m.u.s.|
    010800d0  69 00 63 00 00 00 00 00  00 00 00 00 00 00 00 00  |i.c.............|

=== Really Long filenames ===
    01100640  85 05 a9 6f 20 00 00 00  25 38 48 45 26 38 48 45  |...o ...%8HE&8HE|
    01100650  25 38 48 45 64 64 00 00  00 00 00 00 00 00 00 00  |%8HEdd..........|
    01100660  c0 01 00 32 c6 a5 00 00  d8 52 76 00 00 00 00 00  |...2.....Rv.....|
    01100670  00 00 00 00 14 46 00 00  d8 52 76 00 00 00 00 00  |.....F...Rv.....|
    01100680  c1 00 30 00 30 00 33 00  20 00 2d 00 20 00 4c 00  |..0.0.3. .-. .L.|
    01100690  65 00 64 00 20 00 5a 00  65 00 70 00 70 00 65 00  |e.d. .Z.e.p.p.e.|
    011006a0  c1 00 6c 00 69 00 6e 00  20 00 2d 00 20 00 53 00  |..l.i.n. .-. .S.|
    011006b0  74 00 61 00 69 00 72 00  77 00 61 00 79 00 20 00  |t.a.i.r.w.a.y. .|
    011006c0  c1 00 74 00 6f 00 20 00  68 00 65 00 61 00 76 00  |..t.o. .h.e.a.v.|
    011006d0  65 00 6e 00 20 00 2d 00  20 00 31 00 39 00 37 00  |e.n. .-. .1.9.7.|
    011006e0  c1 00 32 00 2e 00 6d 00  70 00 33 00 00 00 00 00  |..2...m.p.3.....|    
    011006f0  00 00 00 00 00 00 00 00  00 00 00 00 00 00 00 00  |................|

[[Category:Filesystems]]