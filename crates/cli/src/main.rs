mod args;
mod commands;
mod mcp;
mod output;

#[cfg(test)]
mod cli_tests;

use anyhow::Result;
use args::{Cli, Command};
use clap::Parser;
use commands::{
    handle_agent, handle_cancel, handle_conversation, handle_mode, handle_param, handle_proxy,
    handle_search, handle_send,
};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

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
