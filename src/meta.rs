use std::io::{Read, Seek, SeekFrom};

use binrw::BinRead;

const HEADER_OFFSET: u64 = 0x10000;
const OFFSET_XGD3: u64 = 0x2080000;
const OFFSET_XGD2: u64 = 0xFD90000;
pub const SECTOR_SIZE: u32 = 2048;

#[derive(Debug, BinRead)]
#[br(little, magic = b"MICROSOFT*XBOX*MEDIA")]
pub struct IsoMeta {
    pub root_dir_sector: u32,
    pub root_dir_size: u32,
    #[br(ignore)]
    pub root_offset: u64,
}

pub fn get_iso_meta<R: Read + Seek>(reader: &mut R) -> Result<IsoMeta, String> {
    let mut root_offset = OFFSET_XGD2;
    let mut meta;

    reader
        .seek(SeekFrom::Start(root_offset + HEADER_OFFSET))
        .map_err(|e| format!("Error changing read position: {}", e))?;
    meta = IsoMeta::read(reader).ok();

    if meta.is_none() {
        root_offset = OFFSET_XGD3;
        reader
            .seek(SeekFrom::Start(root_offset + HEADER_OFFSET))
            .map_err(|e| format!("Error changing read position: {}", e))?;
        meta = IsoMeta::read(reader).ok();
    }

    if meta.is_none() {
        return Err(format!("Unsupported XISO format"));
    }
    let mut meta = meta.unwrap();
    meta.root_offset = root_offset;

    Ok(meta)
}