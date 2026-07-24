mod args;
mod commands;
mod mcp;
mod output;

#[cfg(test)]
mod cli_tests;

use acp_hub::HubError;
use args::{Cli, Command};
use clap::Parser;
use commands::{
    handle_agent, handle_cancel, handle_conversation, handle_mode, handle_param, handle_proxy,
    handle_search, handle_send,
};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    // Force process exit: RpcClient background tasks can keep the tokio runtime
    // alive after successful commands (named-pipe readers). Phase-1 CLI must
    // still print contracted error codes on failure.
    let code = match run().await {
        Ok(()) => 0u8,
        Err(err) => {
            let line = err
                .downcast_ref::<HubError>()
                .map(HubError::phase1_cli_line)
                .or_else(|| {
                    err.chain()
                        .find_map(|cause| cause.downcast_ref::<HubError>())
                        .map(HubError::phase1_cli_line)
                })
                .unwrap_or_else(|| format!("error: {err}"));
            eprintln!("{line}");
            1
        }
    };
    std::process::exit(code.into());
}

async fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let home = match cli.home {
        Some(home) => home,
        None => acp_hub::home_dir()?,
    };

    match cli.command {
        Command::Serve => acp_hub::daemon::serve(home).await?,
        Command::Agent { command } => handle_agent(&home, command).await?,
        Command::Proxy { command } => handle_proxy(&home, command).await?,
        Command::Conv { command } => handle_conversation(&home, command).await?,
        Command::Send(args) => handle_send(&home, args).await?,
        Command::Param { command } => handle_param(&home, command).await?,
        Command::Mode { command } => handle_mode(&home, command).await?,
        Command::Cancel { conv_id } => handle_cancel(&home, conv_id).await?,
        Command::Search(args) => handle_search(&home, args).await?,
        Command::Mcp => mcp::run(home)
            .await
            .map_err(|err| anyhow::anyhow!("{err}"))?,
    }

    Ok(())
}
