use clap::Parser;
use std::process::ExitCode;

use idm::{
    Args, fake_hardware_client, real_hardware_client_with_model_resolution, run_with_log_level,
};

#[tokio::main]
async fn main() -> ExitCode {
    let args = Args::parse();
    let stdout = std::io::stdout();
    let mut stdout = stdout.lock();

    let run_result = async {
        let log_level = args.log_level();
        let model_resolution = args.model_resolution();
        let (command, maybe_fake_args) = args.into_command_and_fake_args()?;
        let hardware_client = match maybe_fake_args {
            Some(fake_args) => fake_hardware_client(fake_args),
            None => real_hardware_client_with_model_resolution(model_resolution),
        };

        run_with_log_level(command, &mut stdout, hardware_client, log_level).await
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
