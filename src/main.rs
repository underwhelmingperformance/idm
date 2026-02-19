use std::io::IsTerminal;
use std::process::ExitCode;

use clap::Parser;

use idm::{
    Args, OutputFormat, fake_hardware_client, real_hardware_client_with_model_resolution,
    run_with_log_level,
};

#[tokio::main]
async fn main() -> ExitCode {
    let args = Args::parse();
    let mut stdout = std::io::stdout();

    let run_result = async {
        let log_level = args.log_level();
        let output_format = args.output_format().unwrap_or(if stdout.is_terminal() {
            OutputFormat::Pretty
        } else {
            OutputFormat::Json
        });
        let model_resolution = args.model_resolution();
        let (command, maybe_fake_args) = args.into_command_and_fake_args()?;
        let hardware_client = match maybe_fake_args {
            Some(fake_args) => fake_hardware_client(fake_args),
            None => real_hardware_client_with_model_resolution(model_resolution),
        };

        run_with_log_level(
            command,
            &mut stdout,
            hardware_client,
            log_level,
            output_format,
        )
        .await
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
