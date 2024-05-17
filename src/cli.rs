use clap::{Args, Parser};
use std::path::PathBuf;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    #[command(flatten)]
    pub mode: Mode,

    /// Skip System Update if present
    #[arg(short, long)]
    pub skip_update: bool,

    /// Path to the ISO file
    #[arg(name = "iso")]
    pub input: PathBuf,

    /// Output directory or FTP url to extract content to
    #[arg(short, long)]
    pub out: Option<String>,
}

#[derive(Args, Debug)]
#[group(multiple = false)]
pub struct Mode {
    /// Extract content of the ISO file (default)
    #[arg(short = 'x', long)]
    pub extract: bool,

    /// List content of the ISO file
    #[arg(short, long)]
    pub list: bool,
}
