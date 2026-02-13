use clap::Parser;
use std::process::ExitCode;

use idm::{Args, run};

#[tokio::main]
async fn main() -> ExitCode {
    let args = Args::parse();
    let stdout = std::io::stdout();
    let mut stdout = stdout.lock();

    match run(args, &mut stdout).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::from(1)
        }
    }
}
