use std::{path::PathBuf, str::FromStr};

use suppaftp::{FtpStream, Status};
use url_parse::url::Url;

pub struct FtpClient {
    url: Url,
    stream: FtpStream,
}

impl FtpClient {
    pub fn get_path(&self) -> String {
        let url_path = self.url.path_segments().unwrap().join("/");
        return format!("/{}", url_path);
    }

    pub fn connect(url: &str) -> Result<FtpClient, String> {
        let url = url_parse::core::Parser::new(None)
            .parse(url)
            .map_err(|e| format!("Error parsing ftp url {:?}: {}", url, e))?;
        let user = url.username().is_none().then_some("xbox").unwrap();
        let password = url.password().is_none().then_some("xbox").unwrap();

        let mut ftp_stream = FtpStream::connect(
            format!(
                "{}:{}",
                url.host_str().unwrap(),
                url.port_or_known_default().unwrap()
            )
            .as_str(),
        )
        .map_err(|e| format!("Error connecting to ftp server {:?}: {}", &url, e))?;

        ftp_stream
            .login(user, password)
            .map_err(|e| format!("Error connecting to ftp server {:?}: {}", &url, e))?;

        Ok(FtpClient {
            url,
            stream: ftp_stream,
        })
    }

    pub fn create_dir_all(&mut self, path: &str) -> Result<(), String> {
        let segments: Vec<&str> = path.split("/").filter(|s| !s.is_empty()).collect();
        let mut cur_dir = vec![""];

        for segment in segments {
            let dir_exists = self.exists(segment)?;

            if !dir_exists {
                self.mkdir(segment)?;
            }
            cur_dir.push(segment);
            self.cwd(cur_dir.join("/").as_str())?;
        }

        Ok(())
    }

    pub fn exists(&mut self, path: &str) -> Result<bool, String> {
        match self.stream.cwd(path) {
            Ok(_) => return Ok(true),
            Err(e) => match e {
                suppaftp::FtpError::UnexpectedResponse(ref response) => {
                    if response.status == Status::FileUnavailable {
                        return Ok(false);
                    } else {
                        return Err(format!(
                            "Error changing directory '{}' on ftp server: {}",
                            &path, e
                        ));
                    }
                }
                _ => {
                    return Err(format!(
                        "Error changing directory '{}' on ftp server: {}",
                        &path, e
                    ))
                }
            },
        };
    }

    pub fn mkdir(&mut self, path: &str) -> Result<(), String> {
        return self
            .stream
            .mkdir(path)
            .map_err(|e| format!("Error creating directory '{}' on ftp server: {}", path, e));
    }

    pub fn put(&mut self, path: &str) -> Result<impl std::io::Write, String> {
        return self
            .stream
            .put_with_stream(path)
            .map_err(|e| format!("Error opening write stream for file '{}': {}", path, e));
    }

    pub fn put_close(&mut self, writer: impl std::io::Write) -> Result<(), String> {
        return self
            .stream
            .finalize_put_stream(writer)
            .map_err(|e| format!("Error finalizing ftp write stream: {}", e));
    }

    fn cwd(&mut self, path: &str) -> Result<(), String> {
        return self
            .stream
            .cwd(path)
            .map_err(|e| format!("Error changing directory '{}' on ftp server: {}", &path, e));
    }

    pub fn get_file_size(&mut self, out_file: &str) -> Result<i64, String> {
        let file_size: i64;
        match self.stream.size(out_file) {
            Ok(size) => file_size = size as i64,
            Err(e) => match e {
                suppaftp::FtpError::UnexpectedResponse(ref response) => {
                    if response.status == Status::FileUnavailable {
                        file_size = -1;
                    } else {
                        return Err(format!("ftp file size error: {}", e.to_string()));
                    }
                }
                suppaftp::FtpError::BadResponse => {
                    // ftp server bug with integer overflow, use list command
                    let path = PathBuf::from(out_file);
                    let parent = path.parent().unwrap().to_string_lossy();
                    let file = path.file_name().unwrap().to_string_lossy();
                    self.cwd(&parent)?;
                    let list = self
                        .stream
                        .list(None)
                        .map_err(|e| format!("ftp list error: {}", e))?;
                    file_size = list
                        .iter()
                        .map(|e| suppaftp::list::File::from_str(e).unwrap())
                        .filter(|e| e.name() == file)
                        .last()
                        .unwrap()
                        .size() as i64;
                }
                _ => return Err(format!("ftp file size error: {}", e.to_string())),
            },
        };
        Ok(file_size)
    }
}
