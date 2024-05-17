mod entry;
mod ftp;
mod meta;

use std::{
    cmp::min,
    io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write},
    path::PathBuf,
    str::FromStr,
};

use entry::DirEntry;
use ftp::FtpClient;
use meta::{get_iso_meta, IsoMeta, SECTOR_SIZE};
use pbr::{ProgressBar, Units};

const BUFFER_SIZE: u32 = 4096;

#[derive(Copy, Clone, PartialEq)]
pub enum FsMode {
    Local,
    FTP,
}

pub struct XIso {
    file_path: PathBuf,
    reader: BufReader<std::fs::File>,
    meta: IsoMeta,
    fs_mode: FsMode,
    pub root: Vec<DirEntry>,
    ftp_client: Option<FtpClient>,
}

impl XIso {
    pub fn from_path(path: &PathBuf) -> Result<Self, String> {
        let file =
            std::fs::File::open(&path).map_err(|e| format!("Error opening input file: {}", e))?;

        let mut reader = BufReader::new(file);

        let iso_meta = get_iso_meta(&mut reader)?;

        let root_dir = entry::parse_root(&mut reader, &iso_meta)?;

        Ok(XIso {
            file_path: path.clone(),
            reader,
            meta: iso_meta,
            root: root_dir,
            fs_mode: FsMode::Local,
            ftp_client: None,
        })
    }

    pub fn extract_all(&mut self, out_path: &String, skip_update: bool) -> Result<(), String> {
        let mut path = out_path.clone();
        if out_path.starts_with("ftp://") {
            self.fs_mode = FsMode::FTP;
            let ftp_client = FtpClient::connect(out_path)?;
            path = ftp_client.get_path();
            self.ftp_client = Some(ftp_client);          
        }      

        self.create_out_dir(&path)?;  

        let mut entries = self.root.clone();
        if skip_update {
            entries = entries
                .into_iter()
                .filter(|e| e.name != "$SystemUpdate")
                .collect();
        }
        let files_count = self.extract_records(&entries, &path)?;
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

    fn create_out_dir(&mut self, out_path: &String) -> Result<(), String> {
        if self.fs_mode == FsMode::Local {
            let out_path = PathBuf::from_str(out_path)
                .map_err(|e| format!("Error parsing output path '{}': {}", out_path, e))?;
            if out_path.exists() {
                print!("Output dir {:?} already exists. Replacing.", &out_path);
                std::fs::remove_dir_all(&out_path).map_err(|e| {
                    format!("Error deleting output directory {:?}: {}", &out_path, e)
                })?;
            }

            std::fs::create_dir_all(&out_path)
                .map_err(|e| format!("Error creating output directory {:?}: {}", &out_path, e))?;
        } else if self.fs_mode == FsMode::FTP {
            let ftp = self.ftp_client.as_mut().unwrap();
            ftp.create_dir_all(out_path)?;
        }

        Ok(())
    }

    fn dir_exists(&mut self, dir_path: &String) -> Result<bool, String> {
        if self.fs_mode == FsMode::Local {
            let dir_path = PathBuf::from_str(dir_path)
                .map_err(|e| format!("Error parsing output path '{}': {}", dir_path, e))?;
            return Ok(dir_path.exists());
        } else if self.fs_mode == FsMode::FTP {
            let ftp = self.ftp_client.as_mut().unwrap();
            return ftp.exists(dir_path);
        }

        Err(format!("Unsupported mode"))
    }

    fn create_dir(&mut self, dir_path: &String) -> Result<(), String> {
        if self.fs_mode == FsMode::Local {
            return std::fs::create_dir(dir_path)
                .map_err(|e| format!("Error creating output directory {:?}: {}", dir_path, e));
        } else if self.fs_mode == FsMode::FTP {
            let ftp = self.ftp_client.as_mut().unwrap();
            ftp.mkdir(dir_path)?
        }

        Ok(())
    }

    fn extract_records(
        &mut self,
        entries: &Vec<DirEntry>,
        root_path: &String,
    ) -> Result<u32, String> {
        let mut count = 0_u32;
        for entry in entries.iter() {
            if entry.is_dir() {
                let new_dir = format!("{}/{}", root_path, &entry.name);
                if !self.dir_exists(&new_dir)? {
                    self.create_dir(&new_dir)?
                }
                if let Some(entries) = &entry.subdir {
                    count += self.extract_records(entries, &new_dir)?;
                };
            } else {
                self.extract_record(entry, &root_path)?;
                count += 1;
            }
        }
        Ok(count)
    }

    fn extract_record(&mut self, entry: &DirEntry, output_root: &String) -> Result<(), String> {
        let position = self.meta.root_offset + entry.sector as u64 * SECTOR_SIZE as u64;
        self.reader
            .seek(SeekFrom::Start(position))
            .map_err(|_| format!("Unable to jump to record at {}. Broken ISO?", position))?;

        let out_file = format!("{}/{}", output_root, &entry.name);
        let mut file_writer = None;
        let mut ftp_writer = None;

        if self.fs_mode == FsMode::Local {
            let file = std::fs::File::create(&out_file)
                .map_err(|e| format!("Error creating file {:?}: {}", &out_file, e))?;
            file_writer = Some(BufWriter::new(file));
        } else if self.fs_mode == FsMode::FTP {
            let ftp = self.ftp_client.as_mut().unwrap();

            let file_size = ftp.get_file_size(&out_file)?;
            if file_size == -1 {
                ftp_writer = Some(ftp.put(&out_file)?);
            } else if file_size != entry.size as i64 {
                println!("Corrupt remote file: {}, Replacing.", &out_file); // TODO resuming?
                ftp_writer = Some(ftp.put(&out_file)?);
            } else {
                return Ok(());
            }
        }

        let buffer_size = min(entry.size, BUFFER_SIZE);
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
            let ftp = self.ftp_client.as_mut().unwrap();
            ftp.put_close(ftp_writer.unwrap())?;

            let out_file = &out_file;
            let file_size = ftp.get_file_size(&out_file)?;
            if file_size != entry.size as i64 {
                return Err(format!(
                    "File verification failed. {:?} is corrupted.",
                    &out_file
                ));
            }
        } else if self.fs_mode == FsMode::Local {
            file_writer
                .unwrap()
                .flush()
                .map_err(|e| format!("Error flushing file writer: {}", e.to_string()))?;
            let metadata = std::fs::metadata(&out_file).map_err(|e| {
                format!(
                    "Error getting metadata for {:?}: {}",
                    &out_file,
                    e.to_string()
                )
            })?;
            if metadata.len() != entry.size as u64 {
                return Err(format!(
                    "File verification failed. {:?} is corrupted.",
                    &out_file
                ));
            }
        }

        pb.finish_print(format!("{}", &out_file).as_str());
        println!();
        Ok(())
    }
}

fn print_dir(entries: &Vec<DirEntry>, cur_dir: &PathBuf) -> u32 {
    let mut count = 0_u32;
    for entry in entries.iter() {
        if entry.is_dir() {
            let cur_dir = cur_dir.join(&entry.name);
            if let Some(entries) = &entry.subdir {
                count += print_dir(&entries, &cur_dir);
            };
        } else {
            println!("{}", cur_dir.join(&entry.name).to_str().unwrap());
            count += 1;
        }
    }
    count
}
