use std::{
    cmp::min,
    io::{BufReader, BufWriter, Error, Read, Seek, SeekFrom, Write},
    os::unix::fs::MetadataExt,
    path::PathBuf,
    str::FromStr,
};

use pbr::{ProgressBar, Units};
use suppaftp::{FtpStream, Status};
use url::Url;

const OFFSET_XGD3: u64 = 0x2080000;
const OFFSET_XGD2: u64 = 0xFD90000;

#[derive(Copy, Clone, PartialEq)]
pub enum FsMode {
    Local,
    FTP,
}

#[derive(Debug, Eq, Ord, PartialEq, PartialOrd, Clone)]
pub struct Record {
    pub name: String,
    pub sector: u32,
    pub size: u32,
    pub attributes: u8,
    pub subdirectory: Option<Vec<Record>>,
}

#[derive(Debug)]
pub struct IsoMeta {
    pub root_offset: u64,
    pub root_dir_sector: u32,
    pub root_dir_size: u32,
    pub sector_size: u32,
}

pub struct XIso {
    file_path: PathBuf,
    reader: BufReader<std::fs::File>,
    meta: IsoMeta,
    fs_mode: FsMode,
    pub root: Vec<Record>,
    ftp_stream: Option<FtpStream>,
}

impl XIso {
    pub fn from_path(path: &PathBuf) -> Result<Self, String> {
        let file =
            std::fs::File::open(&path).map_err(|e| format!("Error opening input file: {}", e))?;

        let mut reader = BufReader::new(file);

        let iso_meta = get_iso_meta(&mut reader)?;

        let root_dir = parse_dir(
            &mut reader,
            &iso_meta,
            iso_meta.root_dir_size,
            iso_meta.root_dir_sector,
        )
        .map_err(|e| format!("Error parsing ISO file: {}", e))?;

        let parser = XIso {
            file_path: path.clone(),
            reader: reader,
            meta: iso_meta,
            root: root_dir,
            fs_mode: FsMode::Local,
            ftp_stream: None,
        };

        Ok(parser)
    }

    pub fn extract_all(&mut self, out_path: &PathBuf, skip_update: bool) -> Result<(), String> {
        if out_path.starts_with("ftp://") {
            self.fs_mode = FsMode::FTP;

            let url = Url::parse(out_path.to_str().unwrap())
                .map_err(|e| format!("Error parsing ftp url {:?}: {}", &out_path, e.to_string()))?;

            let user = url.username().is_empty().then_some("xbox").unwrap();
            let password = url.password().is_none().then_some("xbox").unwrap();

            let mut ftp_stream = FtpStream::connect(
                format!(
                    "{}:{}",
                    url.host_str().unwrap(),
                    url.port_or_known_default().unwrap()
                )
                .as_str(),
            )
            .map_err(|e: suppaftp::FtpError| {
                format!("Error connecting to ftp server {:?}: {}", &url, e)
            })?;

            ftp_stream
                .login(user, password)
                .map_err(|e| format!("Error connecting to ftp server {:?}: {}", &url, e))?;
            self.ftp_stream = Some(ftp_stream.active_mode());
        }

        self.create_out_dir(out_path)?;

        // TODO how without cloning?
        let mut entries = self.root.clone();
        if skip_update {
            entries = entries
                .into_iter()
                .filter(|e| e.name != "$SystemUpdate")
                .collect();
        }
        let files_count = self.extract_records(&entries, &out_path)?;
        println!("");
        println!("Files extracted: {}", files_count);

        Ok(())
    }

    pub fn list(self) {
        println!("Printing content of {:?}", &self.file_path);
        let path = PathBuf::from("/");
        let files_total = print_dir(&self.root, &path);
        println!();
        println!("Number of files: {}", files_total);
    }

    fn create_out_dir(&mut self, out_path: &PathBuf) -> Result<(), String> {
        if self.fs_mode == FsMode::Local {
            if out_path.exists() {
                print!("Output dir {:?} already exists. Replacing.", &out_path);
                std::fs::remove_dir_all(&out_path).map_err(|e| {
                    format!("Error deleting output directory {:?}: {}", &out_path, e)
                })?;
            }

            std::fs::create_dir_all(&out_path)
                .map_err(|e| format!("Error creating output directory {:?}: {}", &out_path, e))?;
        } else if self.fs_mode == FsMode::FTP {
            let url = Url::parse(out_path.to_str().unwrap())
                .map_err(|e| format!("Error parsing ftp url {:?}: {}", &out_path, e.to_string()))?;

            let segments = match url.path_segments() {
                Some(segments) => segments
                    .map(|v| urlencoding::decode(v).unwrap().into_owned())
                    .collect::<Vec<_>>(),
                None => {
                    return Err(format!("Error parsing url parts of {:?}", &url));
                }
            };

            let ftp_stream = self.ftp_stream.as_mut().unwrap();

            for segment in segments {
                let list = match ftp_stream.list(None) {
                    Ok(list) => list
                        .iter()
                        .map(|entry| suppaftp::list::File::from_str(entry).unwrap())
                        .collect::<Vec<_>>(),
                    Err(err) => {
                        return Err(format!(
                            "Error listing directory on ftp server {:?}: {}",
                            &url.path(),
                            err
                        ));
                    }
                };

                let entry = list.iter().find(|file| file.name() == segment);

                if entry.is_none() {
                    ftp_stream.mkdir(&segment).map_err(|e| {
                        format!(
                            "Error creating directory '{}' on ftp server: {}",
                            &segment, e
                        )
                    })?;
                }

                ftp_stream.cwd(&segment).map_err(|e| {
                    format!(
                        "Error changing directory '{}' on ftp server {:?}: {}",
                        &segment,
                        &url.path(),
                        e
                    )
                })?;
            }
        }

        Ok(())
    }

    fn dir_exists(&mut self, dir_path: &PathBuf) -> Result<bool, String> {
        if self.fs_mode == FsMode::Local {
            return Ok(dir_path.exists());
        } else if self.fs_mode == FsMode::FTP {
            let url = Url::parse(dir_path.to_str().unwrap())
                .map_err(|e| format!("Error parsing ftp url {:?}: {}", &dir_path, e.to_string()))?;

            let server_path = urlencoding::decode(url.path()).unwrap().into_owned();
            let ftp_stream = self.ftp_stream.as_mut().unwrap();

            match ftp_stream.cwd(&server_path) {
                Ok(_) => return Ok(true),
                Err(_) => {
                    // TODO check for FileUnavailable Error
                    return Ok(false);
                }
            };
        }

        Err(format!("Unsupported mode"))
    }

    fn create_dir(&mut self, dir_path: &PathBuf) -> Result<(), String> {
        if self.fs_mode == FsMode::Local {
            return std::fs::create_dir(dir_path)
                .map_err(|e| format!("Error creating output directory {:?}: {}", dir_path, e));
        } else if self.fs_mode == FsMode::FTP {
            let url = Url::parse(dir_path.to_str().unwrap())
                .map_err(|e| format!("Error parsing ftp url {:?}: {}", &dir_path, e.to_string()))?;

            let server_path = urlencoding::decode(url.path()).unwrap().into_owned();
            let ftp_stream = self.ftp_stream.as_mut().unwrap();

            ftp_stream
                .mkdir(&server_path)
                .map_err(|e| format!("Error creating dir {}: {}", &server_path, e.to_string()))?;
        }

        Ok(())
    }

    fn extract_records(
        &mut self,
        entries: &Vec<Record>,
        root_path: &PathBuf,
    ) -> Result<u32, String> {
        let mut count = 0_u32;
        for entry in entries.iter() {
            if entry.is_dir() {
                let new_dir = root_path.join(&entry.name);
                if !self.dir_exists(&new_dir)? {
                    self.create_dir(&new_dir)?
                }
                if let Some(entries) = &entry.subdirectory {
                    count += self.extract_records(entries, &new_dir)?;
                };
            } else {
                self.extract_record(entry, &root_path)?;
                count += 1;
            }
        }
        Ok(count)
    }

    fn extract_record(&mut self, entry: &Record, output_root: &PathBuf) -> Result<(), String> {
        let position = self.meta.root_offset + entry.sector as u64 * self.meta.sector_size as u64;
        self.reader
            .seek(SeekFrom::Start(position))
            .map_err(|_| format!("Unable to jump to record at {}. Broken ISO?", position))?;

        let out_file = output_root.join(&entry.name);
        let mut file_writer = None;
        let mut ftp_writer = None;

        if self.fs_mode == FsMode::Local {
            let file = std::fs::File::create(&out_file)
                .map_err(|e| format!("Error creating file {:?}: {}", &out_file, e))?;
            file_writer = Some(BufWriter::new(file));
        } else if self.fs_mode == FsMode::FTP {
            let url = Url::parse(out_file.to_str().unwrap())
                .map_err(|e| format!("Error parsing ftp url {:?}: {}", &out_file, e.to_string()))?;

            let server_path = urlencoding::decode(url.path()).unwrap().into_owned();
            let ftp_stream = self.ftp_stream.as_mut().unwrap();

            let file_size = ftp_check_file_size(ftp_stream, &out_file)?;
            if file_size == -1 {
                let writer = ftp_stream.put_with_stream(&server_path).map_err(|e| {
                    format!(
                        "Error opening write stream for file {}: {}",
                        &server_path,
                        e.to_string()
                    )
                })?;
                ftp_writer = Some(writer);
            } else if file_size != entry.size as i32 {
                println!("Corrupt remote file: {}, Replacing.", &server_path); // TODO resuming?
                let writer = ftp_stream.put_with_stream(&server_path).map_err(|e| {
                    format!(
                        "Error opening write stream for file {}: {}",
                        &server_path,
                        e.to_string()
                    )
                })?;
                ftp_writer = Some(writer);
            } else {
                return Ok(());
            }
        }

        let buffer_size = min(entry.size, 1024 * 1024) as u32;
        let mut buffer = vec![0; buffer_size as usize];
        let chunk_count = if buffer_size == 0 {
            0
        } else {
            entry.size / buffer_size
        };

        let mut pb = ProgressBar::new(entry.size.into());
        pb.set_units(Units::Bytes);
        pb.message(format!("{}: ", &entry.name).as_str());
        pb.show_speed = false;
        pb.show_time_left = false;

        for _ in 0..chunk_count {
            self.reader
                .read_exact(&mut buffer)
                .map_err(|e| format!("Error reading from ISO file: {}", e))?;

            if self.fs_mode == FsMode::Local {
                file_writer
                    .as_mut()
                    .unwrap()
                    .write_all(&buffer[0..buffer_size as usize])
                    .map_err(|e| format!("Error writing to file {:?}: {}", &out_file, e))?;
            } else if self.fs_mode == FsMode::FTP {
                ftp_writer
                    .as_mut()
                    .unwrap()
                    .write_all(&buffer[0..buffer_size as usize])
                    .map_err(|e| format!("Error writing to ftp file {:?}: {}", &out_file, e))?;
            }
            pb.add(buffer_size as u64);
        }

        if chunk_count > 0 && entry.size % buffer_size != 0 {
            let last_chunk_size = (entry.size - buffer_size * chunk_count) as usize;
            let mut buffer = vec![0; last_chunk_size];
            self.reader
                .read_exact(&mut buffer)
                .map_err(|e| format!("Error reading from ISO file: {}", e))?;

            if self.fs_mode == FsMode::Local {
                file_writer
                    .as_mut()
                    .unwrap()
                    .write_all(&buffer[0..last_chunk_size])
                    .map_err(|e| format!("Error writing to file {:?}: {}", &out_file, e))?;
            } else if self.fs_mode == FsMode::FTP {
                ftp_writer
                    .as_mut()
                    .unwrap()
                    .write_all(&buffer[0..last_chunk_size])
                    .map_err(|e| format!("Error writing to ftp file {:?}: {}", &out_file, e))?;
            }
            pb.add(last_chunk_size as u64);
        }

        if self.fs_mode == FsMode::FTP {
            let ftp_stream = self.ftp_stream.as_mut().unwrap();
            ftp_stream
                .finalize_put_stream(ftp_writer.unwrap())
                .map_err(|e| format!("Error finalizing ftp write stream: {}", e.to_string()))?;

            let file_size = ftp_check_file_size(ftp_stream, &out_file)?;
            if file_size != entry.size as i32 {
                return Err(format!(
                    "File verification failed. {:?} is corrupted.",
                    &out_file
                ));
            }
        } else if self.fs_mode == FsMode::Local {
            file_writer.as_mut().unwrap().flush().map_err(|e| format!("Error flushing file writer: {}", e.to_string()))?;
            let metadata = std::fs::metadata(&out_file).map_err(|e| {
                format!(
                    "Error getting metadata for {:?}: {}",
                    &out_file,
                    e.to_string()
                )
            })?;
            if metadata.size() != entry.size as u64 {
                return Err(format!(
                    "File verification failed. {:?} is corrupted.",
                    &out_file
                ));
            }
        }

        pb.finish_print(format!("{}", &out_file.to_str().unwrap()).as_str());
        println!();
        Ok(())
    }
}

fn ftp_check_file_size(ftp_stream: &mut FtpStream, out_file: &PathBuf) -> Result<i32, String> {
    let url = Url::parse(out_file.to_str().unwrap())
        .map_err(|e| format!("Error parsing ftp url {:?}: {}", &out_file, e.to_string()))?;

    let server_path = urlencoding::decode(url.path()).unwrap().into_owned();

    let mut file_size = -1;
    match ftp_stream.size(&server_path) {
        Ok(size) => file_size = size as i32,
        Err(e) => match e {
            suppaftp::FtpError::ConnectionError(_) => {
                return Err(format!("ftp file size error: {}", e.to_string()));
            }
            suppaftp::FtpError::UnexpectedResponse(ref response) => {
                if response.status == Status::FileUnavailable {
                    file_size = -1;
                } else {
                    return Err(format!("ftp file size error: {}", e.to_string()));
                }
            }
            suppaftp::FtpError::BadResponse => {
                return Err(format!("ftp file size error: {}", e.to_string()));
            }
            suppaftp::FtpError::InvalidAddress(_) => {
                return Err(format!("ftp file size error: {}", e.to_string()));
            }
        },
    };
    Ok(file_size)
}

fn get_iso_meta<R: Read + Seek>(reader: &mut R) -> Result<IsoMeta, String> {
    let root_offset;
    let sector_size = 2048;
    let header_offset: u64 = 0x10000;

    let mut found = check_magic_string(reader, header_offset + OFFSET_XGD2)
        .map_err(|_| format!("Error detecting XISO type"))?;
    if found {
        root_offset = OFFSET_XGD2;
    } else {
        found = check_magic_string(reader, header_offset + OFFSET_XGD3)
            .map_err(|_| format!("Error detecting XISO type"))?;
        if found {
            root_offset = OFFSET_XGD3;
        } else {
            return Err(format!("Unsupported XISO format"));
        }
    }

    let mut buf = [0; 4];
    reader.read_exact(&mut buf).unwrap();
    let root_dir_sector = u32::from_le_bytes(buf);

    reader.read_exact(&mut buf).unwrap();
    let root_dir_size = u32::from_le_bytes(buf);

    Ok(IsoMeta::new(
        root_offset,
        root_dir_sector,
        root_dir_size,
        sector_size,
    ))
}

fn check_magic_string<R: Read + Seek>(reader: &mut R, offset: u64) -> Result<bool, Error> {
    let mut buffer = [0; 20];

    reader.seek(SeekFrom::Start(offset))?;
    reader.read_exact(&mut buffer)?;

    Ok("MICROSOFT*XBOX*MEDIA".as_bytes() == buffer)
}

fn parse_dir<R: Read + Seek>(
    reader: &mut R,
    iso_meta: &IsoMeta,
    size: u32,
    sector: u32,
) -> Result<Vec<Record>, Error> {
    let mut entries = Vec::<Record>::new();

    let mut sector_count = size / iso_meta.sector_size;
    if size % iso_meta.sector_size > 0 {
        sector_count += 1;
    }

    for i in 0..sector_count {
        let sector_position =
            ((sector + i) as u64) * (iso_meta.sector_size as u64) + iso_meta.root_offset;
        reader.seek(SeekFrom::Start(sector_position))?;

        while let Some(entry) = Record::parse(reader, iso_meta)? {
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

fn print_dir(entries: &Vec<Record>, cur_dir: &PathBuf) -> u32 {
    let mut count = 0_u32;
    for entry in entries.iter() {
        if entry.is_dir() {
            let cur_dir = cur_dir.join(&entry.name);
            if let Some(entries) = &entry.subdirectory {
                count += print_dir(&entries, &cur_dir);
            };
        } else {
            println!("{}", cur_dir.join(&entry.name).to_str().unwrap());
            count += 1;
        }
    }
    count
}

impl Record {
    pub fn parse<R: Read + Seek>(
        reader: &mut R,
        iso_meta: &IsoMeta,
    ) -> Result<Option<Record>, Error> {
        let mut buf8 = [0; 1];
        let mut buf16 = [0; 2];
        let mut buf32 = [0; 4];

        reader.read_exact(&mut buf16)?;
        let left = u16::from_le_bytes(buf16);
        reader.read_exact(&mut buf16)?;
        let right = u16::from_le_bytes(buf16);

        if left == 0xffff || right == 0xffff {
            return Ok(None);
        }

        reader.read_exact(&mut buf32)?;
        let sector = u32::from_le_bytes(buf32);
        reader.read_exact(&mut buf32)?;
        let size = u32::from_le_bytes(buf32);

        reader.read_exact(&mut buf8)?;
        let attributes = buf8[0];

        reader.read_exact(&mut buf8)?;
        let name_length = buf8[0];

        let mut name = vec![0; name_length as usize];
        reader.read_exact(&mut name)?;
        let name = String::from_utf8_lossy(&name).into_owned();

        let alignment_mismatch = ((4 - reader.stream_position()? % 4) % 4) as i64;
        reader.seek(SeekFrom::Current(alignment_mismatch))?;

        let is_directory = attributes & 0x10 == 0x10;
        let subdirectory = if is_directory {
            let cur_pos = reader.stream_position()?;
            let subdir = parse_dir(reader, &iso_meta, size, sector)?;
            reader.seek(SeekFrom::Start(cur_pos))?;
            Some(subdir)
        } else {
            None
        };

        Ok(Some(Record {
            sector,
            size,
            attributes,
            name,
            subdirectory,
        }))
    }

    pub fn is_dir(&self) -> bool {
        self.attributes & 0x10 == 0x10
    }
}

impl IsoMeta {
    pub fn new(
        root_offset: u64,
        root_dir_sector: u32,
        root_dir_size: u32,
        sector_size: u32,
    ) -> Self {
        Self {
            root_offset,
            root_dir_sector,
            root_dir_size,
            sector_size,
        }
    }
}
