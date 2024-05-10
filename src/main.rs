use clap::{Args, Parser};
use std::path::PathBuf;
use xiso_ex::XIso;


#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(flatten)]
    mode: Mode,

    /// Skip System Update if present
    #[arg(short, long)]
    skip_update: bool,

    /// Path to the ISO file
    #[arg(name = "iso")]
    input: PathBuf,

    /// Output directory or FTP url to extract content to
    #[arg(short, long)]
    out: Option<PathBuf>,
}

#[derive(Args, Debug)]
#[group(multiple = false)]
struct Mode {
    /// Extract content of the ISO file (default)
    #[arg(short = 'x', long)]
    extract: bool,

    /// List content of the ISO file
    #[arg(short, long)]
    list: bool,
}

fn main() -> Result<(), String> {
    let cli = Cli::parse();

    //let input_path = std::path::absolute(cli.input).unwrap();
    let input_path = cli.input;
    let mode = &cli.mode;
    let skip_update = cli.skip_update;

    let mut xiso = XIso::from_path(&input_path)?;

    if mode.list {
        xiso.list();
        return Ok(());
    }

    let output_path = cli.out.unwrap_or(input_path.with_extension(""));

    println!(
        "Extracting content of {:?} to {:?}",
        &input_path.as_os_str(),
        &output_path
    );

    xiso.extract_all(&output_path, skip_update)?;

    Ok(())
}
