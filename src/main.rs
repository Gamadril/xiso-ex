mod cli;
use clap::Parser;
use xiso_ex::XIso;

fn main() -> Result<(), String> {
    let cli = cli::Cli::parse();

    //let input_path = std::path::absolute(cli.input).unwrap();
    let input_path = cli.input;
    let mode = &cli.mode;
    let skip_update = cli.skip_update;

    let mut xiso = XIso::from_path(&input_path)?;

    if mode.list {
        xiso.list();
        return Ok(());
    }

    let output_path = cli.out.unwrap_or(input_path.with_extension("").to_string_lossy().to_string());

    println!(
        "Extracting content of {:?} to {:?}",
        &input_path.as_os_str(),
        &output_path
    );

    xiso.extract_all(&output_path, skip_update)?;

    Ok(())
}
