use crate::meta::{IsoMeta, SECTOR_SIZE};
use std::io::{Read, Seek, SeekFrom};
use binrw::BinRead;

#[derive(Debug, Eq, Ord, PartialEq, PartialOrd, Clone, BinRead)]
#[br(little)]
pub struct DirEntry {
    pub left_offset: u16,
    pub right_offset: u16,
    pub sector: u32,
    pub size: u32,
    pub attributes: u8,
    name_len: u8,
    #[br(count = name_len)]
    #[br(map = |s: Vec<u8>|String::from_utf8_lossy(&s).to_string(), align_after = 4)]
    pub name: String,
    #[br(ignore)]
    pub subdir: Option<Vec<DirEntry>>,
}

impl DirEntry {
    #[cfg(feature = "alt_parser")]
    pub fn parse<R: Read + Seek>(
        reader: &mut R,
        iso_meta: &IsoMeta,
    ) -> Result<Option<DirEntry>, binrw::Error> {
        let mut record = DirEntry::read(reader)?;

        if record.left_offset == 0xffff || record.right_offset == 0xffff {
            println!("{} - dir empty?", &record.name);
            return Ok(None);
        }

        record.subdir = if record.is_dir() {
            let cur_pos = reader.stream_position()?;
            let subdir = parse_dir(reader, &iso_meta, record.size, record.sector)?;
            reader.seek(SeekFrom::Start(cur_pos))?;
            Some(subdir)
        } else {
            None
        };

        Ok(Some(record))
    }

    pub fn is_dir(&self) -> bool {
        self.attributes & 0x10 == 0x10
    }
}

#[cfg(feature = "alt_parser")]
fn parse_dir<R: Read + Seek>(
    reader: &mut R,
    iso_meta: &IsoMeta,
    size: u32,
    sector: u32,
) -> Result<Vec<DirEntry>, binrw::Error> {
    let mut entries = Vec::<DirEntry>::new();

    let mut sector_count = size / SECTOR_SIZE;
    if size % SECTOR_SIZE > 0 {
        sector_count += 1;
    }

    for i in 0..sector_count {
        let sector_position = ((sector + i) as u64) * (SECTOR_SIZE as u64) + iso_meta.root_offset;
        reader.seek(std::io::SeekFrom::Start(sector_position))?;

        while let Some(entry) = DirEntry::parse(reader, iso_meta)? {
            // TODO duplicates exist, why?
            let exists = entries.iter().find(|e| entry.name == e.name).is_some();
            if !exists {
                entries.push(entry);
            }
        }
    }
    entries.sort_by_key(|rec| rec.name.to_lowercase());

    Ok(entries)
}


#[cfg(feature = "alt_parser")]
pub fn parse_root<R: Read + Seek>(reader: &mut R, iso_meta: &IsoMeta) -> Result<Vec<DirEntry>, String> {
    return parse_dir(
        reader,
        iso_meta,
        iso_meta.root_dir_size,
        iso_meta.root_dir_sector,
    )
    .map_err(|e| format!("Error parsing ISO file: {}", e));
}


#[cfg(not(feature = "alt_parser"))]
fn parse_dir_entry<R: Read + Seek>(
    reader: &mut R,
    iso_meta: &IsoMeta,
    sector: u32,
    offset: u16,
    parent: &mut Vec<DirEntry>,
) -> Result<(), binrw::Error> {
    let sector_position =
        (sector as u64) * (SECTOR_SIZE as u64) + iso_meta.root_offset + offset as u64 * 4;
    reader.seek(SeekFrom::Start(sector_position))?;

    let mut record = DirEntry::read(reader)?;

    record.subdir = if record.is_dir() {
        let mut subdir = Vec::<DirEntry>::new();
        parse_dir_entry(reader, &iso_meta, record.sector, 0, &mut subdir)?;
        Some(subdir)
    } else {
        None
    };

    if record.left_offset != 0 {
        parse_dir_entry(reader, &iso_meta, sector, record.left_offset, parent)?;
    }

    if record.right_offset != 0 {
        parse_dir_entry(reader, &iso_meta, sector, record.right_offset, parent)?;
    }

    parent.push(record);

    Ok(())
}

#[cfg(not(feature = "alt_parser"))]
pub fn parse_root<R: Read + Seek>(reader: &mut R, iso_meta: &IsoMeta) -> Result<Vec<DirEntry>, String> {
    let mut root_dir = Vec::<DirEntry>::new();
    parse_dir_entry(reader, iso_meta, iso_meta.root_dir_sector, 0, &mut root_dir)
        .map_err(|e| format!("Error parsing ISO file: {}", e))?;
    root_dir.sort_by_key(|rec| rec.name.to_lowercase());

    Ok(root_dir)
}
