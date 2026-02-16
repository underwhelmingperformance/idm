use clap::Parser;
use std::process::ExitCode;

use idm::{
    Args, SystemTerminalClient, fake_hardware_client, real_hardware_client, run_with_clients,
};

#[tokio::main]
async fn main() -> ExitCode {
    let args = Args::parse();
    let stdout = std::io::stdout();
    let mut stdout = stdout.lock();

    let run_result = async {
        let (command, maybe_fake_args) = args.into_command_and_fake_args()?;
        let hardware_client = match maybe_fake_args {
            Some(fake_args) => fake_hardware_client(fake_args),
            None => real_hardware_client(),
        };

        run_with_clients(command, &mut stdout, &SystemTerminalClient, hardware_client).await
    }
    .await;

    match run_result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::from(1)
        }
    }
}
