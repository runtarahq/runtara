// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! `runtara-executor`: serves the placement API (see the library), or with
//! `init-pki` makes the deployment's certificates.

use std::path::PathBuf;

const USAGE: &str = "usage:
  runtara-executor
      serve, configured by RUNTARA_EXECUTOR_LISTEN, RUNTARA_EXECUTOR_TLS_DIR,
      RUNTARA_EXECUTOR_STATE_DIR, RUNTARA_EXECUTOR_MAX_RUNS, RUNTARA_EXECUTOR_RUN_BIN
  runtara-executor init-pki --server-dir <dir> --executor-dir <dir> [--replace]
      make a CA and a certificate for each side; each directory is given
      ca.pem, tls.pem and tls.key";

fn init_pki(mut arguments: impl Iterator<Item = String>) -> Result<(), String> {
    let (mut server, mut executor, mut replace) = (None, None, false);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--server-dir" => server = arguments.next().map(PathBuf::from),
            "--executor-dir" => executor = arguments.next().map(PathBuf::from),
            "--replace" => replace = true,
            _ => return Err(USAGE.to_owned()),
        }
    }
    let (Some(server), Some(executor)) = (server, executor) else {
        return Err(USAGE.to_owned());
    };
    runtara_executor::pki::init(&server, &executor, replace)?;
    println!(
        "wrote the server's certificate to {} and the executor's to {}",
        server.display(),
        executor.display()
    );
    Ok(())
}

async fn serve() -> Result<(), String> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();
    let config = runtara_executor::Config::from_env()?;
    let executor = runtara_executor::Executor::bind(&config).await?;
    tracing::info!(
        address = %executor.address(),
        max_runs = config.max_runs,
        "executor listening"
    );
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .map_err(|error| error.to_string())?;
    tokio::select! {
        () = executor.serve() => {}
        _ = terminate.recv() => {}
        _ = tokio::signal::ctrl_c() => {}
    }
    // Its runs end with it: each run's process exits when its socket closes,
    // and the server recovers a run whose executor went away.
    Ok(())
}

#[tokio::main]
async fn main() {
    let mut arguments = std::env::args().skip(1);
    let result = match arguments.next().as_deref() {
        None => serve().await,
        Some("init-pki") => init_pki(arguments),
        Some(_) => Err(USAGE.to_owned()),
    };
    if let Err(error) = result {
        eprintln!("runtara-executor: {error}");
        std::process::exit(2);
    }
}
