//! On-disk exFAT structures (spec rev 1.00).
//!
//! Multi-byte fields are little-endian. Directory entries are 32 bytes.

use crate::{Error, Result};

pub const DIR_ENTRY_SIZE: usize = 32;
pub const BOOT_SECTOR_SIZE: usize = 512;
pub const BOOT_REGION_SECTORS: u32 = 11;
pub const BACKUP_BOOT_SECTOR: u32 = 12;
pub const OEM_PARAM_COUNT: usize = 10;
pub const OEM_PARAM_SIZE: usize = 48;
pub const FILE_NAME_CHARS_PER_ENTRY: usize = 15;

pub const FAT_FREE: u32 = 0x0000_0000;
pub const FAT_BAD: u32 = 0xFFFF_FFF7;
pub const FAT_END: u32 = 0xFFFF_FFFF;
pub const FAT_MEDIA: u32 = 0xFFFF_FFF8;

pub const FS_NAME: &[u8; 8] = b"EXFAT   ";
pub const BOOT_SIGNATURE: u16 = 0xAA55;
pub const EXTENDED_BOOT_SIGNATURE: u32 = 0xAA55_0000;

/// `{0A0C7E46-3399-4021-90C8-FA6D389C4BA2}`
pub const FLASH_PARAMETERS_GUID: Guid = Guid([
    0x46, 0x7E, 0x0C, 0x0A, 0x99, 0x33, 0x21, 0x40, 0x90, 0xC8, 0xFA, 0x6D, 0x38, 0x9C, 0x4B, 0xA2,
]);
pub const NULL_GUID: Guid = Guid([0; 16]);

#[inline]
fn u16_at(buf: &[u8], off: usize) -> u16 {
    u16::from_le_bytes(buf[off..off + 2].try_into().unwrap())
}

#[inline]
fn u32_at(buf: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(buf[off..off + 4].try_into().unwrap())
}

#[inline]
fn u64_at(buf: &[u8], off: usize) -> u64 {
    u64::from_le_bytes(buf[off..off + 8].try_into().unwrap())
}

fn need<'a>(buf: &'a [u8], n: usize, what: &str) -> Result<&'a [u8]> {
    if buf.len() < n {
        return Err(Error::invalid(format!(
            "{what} truncated (have {} bytes, need {n})",
            buf.len()
        )));
    }
    Ok(&buf[..n])
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Guid(pub [u8; 16]);

impl Guid {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let bytes = need(bytes, 16, "GUID")?;
        Ok(Guid(bytes.try_into().unwrap()))
    }

    pub fn is_null(self) -> bool {
        self == NULL_GUID
    }
}

/// Table 5. `VolumeFlags` at boot sector offset 106.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VolumeFlags(pub u16);

impl VolumeFlags {
    pub const ACTIVE_FAT: u16 = 1 << 0;
    pub const VOLUME_DIRTY: u16 = 1 << 1;
    pub const MEDIA_FAILURE: u16 = 1 << 2;
    pub const CLEAR_TO_ZERO: u16 = 1 << 3;

    pub fn active_fat(self) -> u8 {
        (self.0 & Self::ACTIVE_FAT) as u8
    }

    pub fn volume_dirty(self) -> bool {
        self.0 & Self::VOLUME_DIRTY != 0
    }

    pub fn media_failure(self) -> bool {
        self.0 & Self::MEDIA_FAILURE != 0
    }

    pub fn clear_to_zero(self) -> bool {
        self.0 & Self::CLEAR_TO_ZERO != 0
    }
}

/// Table 4. Main and Backup Boot Sector (first 512 bytes).
#[derive(Clone, Debug)]
pub struct BootSector {
    pub jump_boot: [u8; 3],
    pub file_system_name: [u8; 8],
    pub must_be_zero: [u8; 53],
    pub partition_offset: u64,
    pub volume_length: u64,
    pub fat_offset: u32,
    pub fat_length: u32,
    pub cluster_heap_offset: u32,
    pub cluster_count: u32,
    pub first_cluster_of_root_directory: u32,
    pub volume_serial_number: u32,
    pub file_system_revision: u16,
    pub volume_flags: VolumeFlags,
    pub bytes_per_sector_shift: u8,
    pub sectors_per_cluster_shift: u8,
    pub number_of_fats: u8,
    pub drive_select: u8,
    pub percent_in_use: u8,
    pub reserved: [u8; 7],
    pub boot_code: [u8; 390],
    pub boot_signature: u16,
}

impl BootSector {
    pub fn parse(sector: &[u8]) -> Result<Self> {
        let b = need(sector, BOOT_SECTOR_SIZE, "boot sector")?;
        let mut jump_boot = [0u8; 3];
        jump_boot.copy_from_slice(&b[0..3]);
        let mut file_system_name = [0u8; 8];
        file_system_name.copy_from_slice(&b[3..11]);
        let mut must_be_zero = [0u8; 53];
        must_be_zero.copy_from_slice(&b[11..64]);
        let mut reserved = [0u8; 7];
        reserved.copy_from_slice(&b[113..120]);
        let mut boot_code = [0u8; 390];
        boot_code.copy_from_slice(&b[120..510]);
        Ok(Self {
            jump_boot,
            file_system_name,
            must_be_zero,
            partition_offset: u64_at(b, 64),
            volume_length: u64_at(b, 72),
            fat_offset: u32_at(b, 80),
            fat_length: u32_at(b, 84),
            cluster_heap_offset: u32_at(b, 88),
            cluster_count: u32_at(b, 92),
            first_cluster_of_root_directory: u32_at(b, 96),
            volume_serial_number: u32_at(b, 100),
            file_system_revision: u16_at(b, 104),
            volume_flags: VolumeFlags(u16_at(b, 106)),
            bytes_per_sector_shift: b[108],
            sectors_per_cluster_shift: b[109],
            number_of_fats: b[110],
            drive_select: b[111],
            percent_in_use: b[112],
            reserved,
            boot_code,
            boot_signature: u16_at(b, 510),
        })
    }

    pub fn validate(&self) -> Result<()> {
        if &self.file_system_name != FS_NAME {
            return Err(Error::invalid(format!(
                "not an exFAT volume (FileSystemName = {:?})",
                String::from_utf8_lossy(&self.file_system_name)
            )));
        }
        if self.boot_signature != BOOT_SIGNATURE {
            return Err(Error::invalid(format!(
                "bad boot signature {:#06x}",
                self.boot_signature
            )));
        }
        if self.must_be_zero.iter().any(|&b| b != 0) {
            return Err(Error::invalid("MustBeZero region is not zero"));
        }
        if !(9..=12).contains(&self.bytes_per_sector_shift) {
            return Err(Error::invalid(format!(
                "BytesPerSectorShift {} out of range",
                self.bytes_per_sector_shift
            )));
        }
        let max_spc = 25u8.saturating_sub(self.bytes_per_sector_shift);
        if self.sectors_per_cluster_shift > max_spc {
            return Err(Error::invalid(format!(
                "SectorsPerClusterShift {} out of range",
                self.sectors_per_cluster_shift
            )));
        }
        if self.number_of_fats != 1 && self.number_of_fats != 2 {
            return Err(Error::invalid(format!(
                "NumberOfFats {} is invalid",
                self.number_of_fats
            )));
        }
        if self.volume_flags.active_fat() == 1 && self.number_of_fats != 2 {
            return Err(Error::invalid("ActiveFat is 1 but NumberOfFats is not 2"));
        }
        let major = (self.file_system_revision >> 8) as u8;
        if major != 1 {
            return Err(Error::invalid(format!(
                "unsupported exFAT major revision {major}"
            )));
        }
        if self.cluster_count == 0 {
            return Err(Error::invalid("ClusterCount is 0"));
        }
        if self.first_cluster_of_root_directory < 2
            || self.first_cluster_of_root_directory > self.cluster_count + 1
        {
            return Err(Error::invalid("FirstClusterOfRootDirectory out of range"));
        }
        if self.fat_offset < 24 {
            return Err(Error::invalid("FatOffset < 24"));
        }
        Ok(())
    }

    pub fn bytes_per_sector(&self) -> u32 {
        1u32 << self.bytes_per_sector_shift
    }

    pub fn sectors_per_cluster(&self) -> u32 {
        1u32 << self.sectors_per_cluster_shift
    }

    pub fn cluster_size(&self) -> u32 {
        self.bytes_per_sector() * self.sectors_per_cluster()
    }

    pub fn major_revision(&self) -> u8 {
        (self.file_system_revision >> 8) as u8
    }

    pub fn minor_revision(&self) -> u8 {
        (self.file_system_revision & 0xFF) as u8
    }

    pub fn last_cluster(&self) -> u32 {
        self.cluster_count + 1
    }
}

/// Spec Figure 1. Checksum of the 11-sector Main or Backup Boot region.
/// Bytes 106, 107 (`VolumeFlags`) and 112 (`PercentInUse`) are skipped.
pub fn boot_checksum(sectors: &[u8], bytes_per_sector: u32) -> u32 {
    let n = bytes_per_sector as usize * BOOT_REGION_SECTORS as usize;
    let n = n.min(sectors.len());
    let mut checksum: u32 = 0;
    for (index, &byte) in sectors.iter().take(n).enumerate() {
        if index == 106 || index == 107 || index == 112 {
            continue;
        }
        checksum = ((if checksum & 1 != 0 { 0x8000_0000 } else { 0 }) + (checksum >> 1))
            .wrapping_add(byte as u32);
    }
    checksum
}

/// Table 6. One Extended Boot Sector (sectors 1–8 / 13–20).
#[derive(Clone, Debug)]
pub struct ExtendedBootSector {
    pub extended_boot_code: Vec<u8>,
    pub extended_boot_signature: u32,
}

impl ExtendedBootSector {
    pub fn parse(sector: &[u8]) -> Result<Self> {
        if sector.len() < 4 {
            return Err(Error::invalid("extended boot sector truncated"));
        }
        let sig_off = sector.len() - 4;
        Ok(Self {
            extended_boot_code: sector[..sig_off].to_vec(),
            extended_boot_signature: u32_at(sector, sig_off),
        })
    }

    pub fn is_valid(&self) -> bool {
        self.extended_boot_signature == EXTENDED_BOOT_SIGNATURE
    }
}

/// Table 8. Generic OEM Parameters template (48 bytes).
#[derive(Clone, Copy, Debug)]
pub struct GenericParameters {
    pub parameters_guid: Guid,
    pub custom_defined: [u8; 32],
}

impl GenericParameters {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let b = need(bytes, OEM_PARAM_SIZE, "OEM parameters")?;
        let mut custom_defined = [0u8; 32];
        custom_defined.copy_from_slice(&b[16..48]);
        Ok(Self {
            parameters_guid: Guid::from_bytes(&b[0..16])?,
            custom_defined,
        })
    }

    pub fn is_null(&self) -> bool {
        self.parameters_guid.is_null()
    }

    pub fn as_flash(&self) -> Option<FlashParameters> {
        if self.parameters_guid == FLASH_PARAMETERS_GUID {
            Some(FlashParameters::from_generic(self))
        } else {
            None
        }
    }
}

/// Table 10. Flash Parameters (GUID `FLASH_PARAMETERS_GUID`).
#[derive(Clone, Copy, Debug)]
pub struct FlashParameters {
    pub parameters_guid: Guid,
    pub erase_block_size: u32,
    pub page_size: u32,
    pub spare_sectors: u32,
    pub random_access_time: u32,
    pub programming_time: u32,
    pub read_cycle: u32,
    pub write_cycle: u32,
}

impl FlashParameters {
    fn from_generic(g: &GenericParameters) -> Self {
        let c = &g.custom_defined;
        Self {
            parameters_guid: g.parameters_guid,
            erase_block_size: u32_at(c, 0),
            page_size: u32_at(c, 4),
            spare_sectors: u32_at(c, 8),
            random_access_time: u32_at(c, 12),
            programming_time: u32_at(c, 16),
            read_cycle: u32_at(c, 20),
            write_cycle: u32_at(c, 24),
        }
    }
}

/// Table 7. OEM Parameters sub-region: ten 48-byte slots.
#[derive(Clone, Debug)]
pub struct OemParameters {
    pub parameters: [GenericParameters; OEM_PARAM_COUNT],
}

impl OemParameters {
    pub fn parse(sector: &[u8]) -> Result<Self> {
        if sector.len() < OEM_PARAM_COUNT * OEM_PARAM_SIZE {
            return Err(Error::invalid("OEM Parameters sector too small"));
        }
        let mut parameters = [GenericParameters {
            parameters_guid: NULL_GUID,
            custom_defined: [0; 32],
        }; OEM_PARAM_COUNT];
        for (i, slot) in parameters.iter_mut().enumerate() {
            let off = i * OEM_PARAM_SIZE;
            *slot = GenericParameters::parse(&sector[off..off + OEM_PARAM_SIZE])?;
        }
        Ok(Self { parameters })
    }
}

/// Table 15. Generic `EntryType` byte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EntryType(pub u8);

impl EntryType {
    pub const END: u8 = 0x00;
    pub const ALLOC_BITMAP: u8 = 0x81;
    pub const UPCASE_TABLE: u8 = 0x82;
    pub const VOLUME_LABEL: u8 = 0x83;
    pub const FILE: u8 = 0x85;
    pub const VOLUME_GUID: u8 = 0xA0;
    pub const TEXFAT_PADDING: u8 = 0xA1;
    pub const STREAM_EXTENSION: u8 = 0xC0;
    pub const FILE_NAME: u8 = 0xC1;
    pub const VENDOR_EXTENSION: u8 = 0xE0;
    pub const VENDOR_ALLOCATION: u8 = 0xE1;

    pub fn is_end(self) -> bool {
        self.0 == Self::END
    }

    pub fn in_use(self) -> bool {
        self.0 & 0x80 != 0
    }

    pub fn type_code(self) -> u8 {
        self.0 & 0x1F
    }

    pub fn is_critical(self) -> bool {
        self.0 & 0x20 == 0
    }

    pub fn is_primary(self) -> bool {
        self.0 & 0x40 == 0
    }

    pub fn is_secondary(self) -> bool {
        !self.is_primary()
    }
}

/// Table 17 / 18. `GeneralPrimaryFlags` / `GeneralSecondaryFlags`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GeneralFlags(pub u8);

impl GeneralFlags {
    pub const ALLOCATION_POSSIBLE: u8 = 1 << 0;
    pub const NO_FAT_CHAIN: u8 = 1 << 1;

    pub fn allocation_possible(self) -> bool {
        self.0 & Self::ALLOCATION_POSSIBLE != 0
    }

    pub fn no_fat_chain(self) -> bool {
        self.0 & Self::NO_FAT_CHAIN != 0
    }
}

/// Table 28. `FileAttributes`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FileAttributes(pub u16);

impl FileAttributes {
    pub const READ_ONLY: u16 = 1 << 0;
    pub const HIDDEN: u16 = 1 << 1;
    pub const SYSTEM: u16 = 1 << 2;
    pub const DIRECTORY: u16 = 1 << 4;
    pub const ARCHIVE: u16 = 1 << 5;

    pub fn is_directory(self) -> bool {
        self.0 & Self::DIRECTORY != 0
    }
}

/// Table 29. Timestamp packed into a `u32`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Timestamp(pub u32);

impl Timestamp {
    pub fn double_seconds(self) -> u8 {
        (self.0 & 0x1F) as u8
    }

    pub fn minute(self) -> u8 {
        ((self.0 >> 5) & 0x3F) as u8
    }

    pub fn hour(self) -> u8 {
        ((self.0 >> 11) & 0x1F) as u8
    }

    pub fn day(self) -> u8 {
        ((self.0 >> 16) & 0x1F) as u8
    }

    pub fn month(self) -> u8 {
        ((self.0 >> 21) & 0x0F) as u8
    }

    pub fn year(self) -> u16 {
        1980 + ((self.0 >> 25) & 0x7F) as u16
    }
}

/// Table 30. UTC offset: 15-minute increments, bit 7 = valid.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UtcOffset(pub u8);

impl UtcOffset {
    pub fn is_valid(self) -> bool {
        self.0 & 0x80 != 0
    }

    /// Offset from UTC in 15-minute units (signed 7-bit).
    pub fn offset_units(self) -> i8 {
        let v = self.0 & 0x7F;
        if v & 0x40 != 0 {
            (v | 0x80) as i8
        } else {
            v as i8
        }
    }
}

/// Raw 32-byte directory entry.
#[derive(Clone, Copy, Debug)]
pub struct RawDirEntry(pub [u8; DIR_ENTRY_SIZE]);

impl RawDirEntry {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let b = need(bytes, DIR_ENTRY_SIZE, "directory entry")?;
        Ok(Self(b.try_into().unwrap()))
    }

    pub fn entry_type(self) -> EntryType {
        EntryType(self.0[0])
    }

    pub fn first_cluster(self) -> u32 {
        u32_at(&self.0, 20)
    }

    pub fn data_length(self) -> u64 {
        u64_at(&self.0, 24)
    }
}

/// Table 14. Generic DirectoryEntry template.
#[derive(Clone, Copy, Debug)]
pub struct GenericDirEntry {
    pub entry_type: EntryType,
    pub custom_defined: [u8; 19],
    pub first_cluster: u32,
    pub data_length: u64,
}

impl GenericDirEntry {
    pub fn from_raw(raw: RawDirEntry) -> Self {
        let mut custom_defined = [0u8; 19];
        custom_defined.copy_from_slice(&raw.0[1..20]);
        Self {
            entry_type: raw.entry_type(),
            custom_defined,
            first_cluster: raw.first_cluster(),
            data_length: raw.data_length(),
        }
    }
}

/// Table 16. Generic Primary DirectoryEntry template.
#[derive(Clone, Copy, Debug)]
pub struct GenericPrimaryDirEntry {
    pub entry_type: EntryType,
    pub secondary_count: u8,
    pub set_checksum: u16,
    pub general_primary_flags: GeneralFlags,
    pub custom_defined: [u8; 14],
    pub first_cluster: u32,
    pub data_length: u64,
}

impl GenericPrimaryDirEntry {
    pub fn from_raw(raw: RawDirEntry) -> Self {
        let mut custom_defined = [0u8; 14];
        custom_defined.copy_from_slice(&raw.0[6..20]);
        Self {
            entry_type: raw.entry_type(),
            secondary_count: raw.0[1],
            set_checksum: u16_at(&raw.0, 2),
            general_primary_flags: GeneralFlags(u16_at(&raw.0, 4) as u8),
            custom_defined,
            first_cluster: raw.first_cluster(),
            data_length: raw.data_length(),
        }
    }
}

/// Table 18. Generic Secondary DirectoryEntry template.
#[derive(Clone, Copy, Debug)]
pub struct GenericSecondaryDirEntry {
    pub entry_type: EntryType,
    pub general_secondary_flags: GeneralFlags,
    pub custom_defined: [u8; 18],
    pub first_cluster: u32,
    pub data_length: u64,
}

impl GenericSecondaryDirEntry {
    pub fn from_raw(raw: RawDirEntry) -> Self {
        let mut custom_defined = [0u8; 18];
        custom_defined.copy_from_slice(&raw.0[2..20]);
        Self {
            entry_type: raw.entry_type(),
            general_secondary_flags: GeneralFlags(raw.0[1]),
            custom_defined,
            first_cluster: raw.first_cluster(),
            data_length: raw.data_length(),
        }
    }
}

/// Table 20. Allocation Bitmap directory entry (`0x81`).
#[derive(Clone, Copy, Debug)]
pub struct AllocationBitmapDirEntry {
    pub entry_type: EntryType,
    pub bitmap_flags: u8,
    pub first_cluster: u32,
    pub data_length: u64,
}

impl AllocationBitmapDirEntry {
    pub fn from_raw(raw: RawDirEntry) -> Self {
        Self {
            entry_type: raw.entry_type(),
            bitmap_flags: raw.0[1],
            first_cluster: raw.first_cluster(),
            data_length: raw.data_length(),
        }
    }

    pub fn bitmap_identifier(self) -> u8 {
        self.bitmap_flags & 1
    }
}

/// Table 23. Up-case Table directory entry (`0x82`).
#[derive(Clone, Copy, Debug)]
pub struct UpcaseTableDirEntry {
    pub entry_type: EntryType,
    pub table_checksum: u32,
    pub first_cluster: u32,
    pub data_length: u64,
}

impl UpcaseTableDirEntry {
    pub fn from_raw(raw: RawDirEntry) -> Self {
        Self {
            entry_type: raw.entry_type(),
            table_checksum: u32_at(&raw.0, 4),
            first_cluster: raw.first_cluster(),
            data_length: raw.data_length(),
        }
    }
}

/// Table 26. Volume Label directory entry (`0x83`).
#[derive(Clone, Copy, Debug)]
pub struct VolumeLabelDirEntry {
    pub entry_type: EntryType,
    pub character_count: u8,
    pub volume_label: [u16; 11],
}

impl VolumeLabelDirEntry {
    pub fn from_raw(raw: RawDirEntry) -> Self {
        let mut volume_label = [0u16; 11];
        for (i, ch) in volume_label.iter_mut().enumerate() {
            *ch = u16_at(&raw.0, 2 + i * 2);
        }
        Self {
            entry_type: raw.entry_type(),
            character_count: raw.0[1],
            volume_label,
        }
    }

    pub fn label(&self) -> String {
        let n = self.character_count.min(11) as usize;
        String::from_utf16_lossy(&self.volume_label[..n])
    }
}

/// Table 27. File directory entry (`0x85`).
#[derive(Clone, Copy, Debug)]
pub struct FileDirEntry {
    pub entry_type: EntryType,
    pub secondary_count: u8,
    pub set_checksum: u16,
    pub file_attributes: FileAttributes,
    pub create_timestamp: Timestamp,
    pub last_modified_timestamp: Timestamp,
    pub last_accessed_timestamp: Timestamp,
    pub create_10ms_increment: u8,
    pub last_modified_10ms_increment: u8,
    pub create_utc_offset: UtcOffset,
    pub last_modified_utc_offset: UtcOffset,
    pub last_accessed_utc_offset: UtcOffset,
}

impl FileDirEntry {
    pub fn from_raw(raw: RawDirEntry) -> Self {
        Self {
            entry_type: raw.entry_type(),
            secondary_count: raw.0[1],
            set_checksum: u16_at(&raw.0, 2),
            file_attributes: FileAttributes(u16_at(&raw.0, 4)),
            create_timestamp: Timestamp(u32_at(&raw.0, 8)),
            last_modified_timestamp: Timestamp(u32_at(&raw.0, 12)),
            last_accessed_timestamp: Timestamp(u32_at(&raw.0, 16)),
            create_10ms_increment: raw.0[20],
            last_modified_10ms_increment: raw.0[21],
            create_utc_offset: UtcOffset(raw.0[22]),
            last_modified_utc_offset: UtcOffset(raw.0[23]),
            last_accessed_utc_offset: UtcOffset(raw.0[24]),
        }
    }
}

/// Table 32. Volume GUID directory entry (`0xA0`).
#[derive(Clone, Copy, Debug)]
pub struct VolumeGuidDirEntry {
    pub entry_type: EntryType,
    pub secondary_count: u8,
    pub set_checksum: u16,
    pub general_primary_flags: GeneralFlags,
    pub volume_guid: Guid,
}

impl VolumeGuidDirEntry {
    pub fn from_raw(raw: RawDirEntry) -> Self {
        Self {
            entry_type: raw.entry_type(),
            secondary_count: raw.0[1],
            set_checksum: u16_at(&raw.0, 2),
            general_primary_flags: GeneralFlags(u16_at(&raw.0, 4) as u8),
            volume_guid: Guid(raw.0[6..22].try_into().unwrap()),
        }
    }
}

/// Table 33. Stream Extension directory entry (`0xC0`).
#[derive(Clone, Copy, Debug)]
pub struct StreamExtensionDirEntry {
    pub entry_type: EntryType,
    pub general_secondary_flags: GeneralFlags,
    pub name_length: u8,
    pub name_hash: u16,
    pub valid_data_length: u64,
    pub first_cluster: u32,
    pub data_length: u64,
}

impl StreamExtensionDirEntry {
    pub fn from_raw(raw: RawDirEntry) -> Self {
        Self {
            entry_type: raw.entry_type(),
            general_secondary_flags: GeneralFlags(raw.0[1]),
            name_length: raw.0[3],
            name_hash: u16_at(&raw.0, 4),
            valid_data_length: u64_at(&raw.0, 8),
            first_cluster: raw.first_cluster(),
            data_length: raw.data_length(),
        }
    }
}

/// Table 34. File Name directory entry (`0xC1`).
#[derive(Clone, Copy, Debug)]
pub struct FileNameDirEntry {
    pub entry_type: EntryType,
    pub general_secondary_flags: GeneralFlags,
    pub file_name: [u16; FILE_NAME_CHARS_PER_ENTRY],
}

impl FileNameDirEntry {
    pub fn from_raw(raw: RawDirEntry) -> Self {
        let mut file_name = [0u16; FILE_NAME_CHARS_PER_ENTRY];
        for (i, ch) in file_name.iter_mut().enumerate() {
            *ch = u16_at(&raw.0, 2 + i * 2);
        }
        Self {
            entry_type: raw.entry_type(),
            general_secondary_flags: GeneralFlags(raw.0[1]),
            file_name,
        }
    }
}

/// Table 36. Vendor Extension directory entry (`0xE0`).
#[derive(Clone, Copy, Debug)]
pub struct VendorExtensionDirEntry {
    pub entry_type: EntryType,
    pub general_secondary_flags: GeneralFlags,
    pub vendor_guid: Guid,
    pub vendor_defined: [u8; 14],
}

impl VendorExtensionDirEntry {
    pub fn from_raw(raw: RawDirEntry) -> Self {
        let mut vendor_defined = [0u8; 14];
        vendor_defined.copy_from_slice(&raw.0[18..32]);
        Self {
            entry_type: raw.entry_type(),
            general_secondary_flags: GeneralFlags(raw.0[1]),
            vendor_guid: Guid(raw.0[2..18].try_into().unwrap()),
            vendor_defined,
        }
    }
}

/// Table 37. Vendor Allocation directory entry (`0xE1`).
#[derive(Clone, Copy, Debug)]
pub struct VendorAllocationDirEntry {
    pub entry_type: EntryType,
    pub general_secondary_flags: GeneralFlags,
    pub vendor_guid: Guid,
    pub vendor_defined: u16,
    pub first_cluster: u32,
    pub data_length: u64,
}

impl VendorAllocationDirEntry {
    pub fn from_raw(raw: RawDirEntry) -> Self {
        Self {
            entry_type: raw.entry_type(),
            general_secondary_flags: GeneralFlags(raw.0[1]),
            vendor_guid: Guid(raw.0[2..18].try_into().unwrap()),
            vendor_defined: u16_at(&raw.0, 18),
            first_cluster: raw.first_cluster(),
            data_length: raw.data_length(),
        }
    }
}

/// Spec Figure 2. Entry-set checksum (skips bytes 2–3 of the first entry).
pub fn entry_set_checksum(entries: &[u8]) -> u16 {
    let mut checksum: u16 = 0;
    for (index, &byte) in entries.iter().enumerate() {
        if index == 2 || index == 3 {
            continue;
        }
        checksum = ((if checksum & 1 != 0 { 0x8000 } else { 0 }) + (checksum >> 1))
            .wrapping_add(byte as u16);
    }
    checksum
}

/// A File directory entry set: File + Stream Extension + File Name(s) + optional vendor entries.
#[derive(Clone, Debug)]
pub struct FileEntrySet {
    pub file: FileDirEntry,
    pub stream: StreamExtensionDirEntry,
    pub name: String,
    pub vendor_extensions: Vec<VendorExtensionDirEntry>,
    pub vendor_allocations: Vec<VendorAllocationDirEntry>,
}

impl FileEntrySet {
    pub fn is_directory(&self) -> bool {
        self.file.file_attributes.is_directory()
    }

    pub fn no_fat_chain(&self) -> bool {
        self.stream.general_secondary_flags.no_fat_chain()
    }

    pub fn first_cluster(&self) -> u32 {
        self.stream.first_cluster
    }

    pub fn data_length(&self) -> u64 {
        self.stream.data_length
    }
}

pub fn decode_file_name(name_entries: &[FileNameDirEntry], name_length: u8) -> String {
    let mut units = Vec::with_capacity(name_length as usize);
    for entry in name_entries {
        units.extend_from_slice(&entry.file_name);
    }
    units.truncate(name_length as usize);
    String::from_utf16_lossy(&units)
}
