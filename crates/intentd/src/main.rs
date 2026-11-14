//! `intentd` binary: logging, runtime, signals and systemd readiness around [`intentd::Daemon`].

use std::process::ExitCode;
use std::time::Duration;

use intentd::{Daemon, DaemonOptions};
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

/// systemd `RestartPreventExitStatus=78` (EX_CONFIG): restarting cannot help.
const EX_CONFIG: u8 = 78;

fn init_tracing() {
    let filter = EnvFilter::try_from_env("INTENT_LOG")
        .or_else(|_| EnvFilter::try_from_default_env())
        .unwrap_or_else(|_| EnvFilter::new("info,hyper=warn,reqwest=warn,zbus=warn,notify=warn"));
    // Under systemd, stderr goes to the journal, which timestamps and rotates; plain
    // text without ANSI colours keeps `journalctl` readable. No log files are written.
    let journald = std::env::var_os("JOURNAL_STREAM").is_some();
    let builder = tracing_subscriber::fmt().with_env_filter(filter).with_writer(std::io::stderr).with_target(false);
    if journald {
        builder.with_ansi(false).without_time().init();
    } else {
        builder.init();
    }
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
                return;
            }
        };
        tokio::select! {
            _ = term.recv() => {}
            _ = tokio::signal::ctrl_c() => {}
        }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutdown requested");
    let _ = sd_notify::notify(false, &[sd_notify::NotifyState::Stopping]);
}

async fn run() -> Result<(), ExitCode> {
    let opts = DaemonOptions::from_env().map_err(|err| {
        tracing::error!(error = %err, "invalid configuration");
        ExitCode::from(EX_CONFIG)
    })?;
    let listener = TcpListener::bind(opts.bind).await.map_err(|err| {
        tracing::error!(error = %err, addr = %opts.bind, "cannot bind; is another intentd running?");
        ExitCode::from(EX_CONFIG)
    })?;
    let daemon = Daemon::start(opts.clone()).await.map_err(|err| {
        tracing::error!(error = format!("{err:#}"), "startup failed");
        ExitCode::FAILURE
    })?;
    tracing::info!(addr = %opts.bind, version = env!("CARGO_PKG_VERSION"), token = %daemon.token_path.display(), "intentd ready");
    let _ = sd_notify::notify(false, &[sd_notify::NotifyState::Ready]);
    daemon.serve(listener, shutdown_signal()).await.map_err(|err| {
        tracing::error!(error = format!("{err:#}"), "server stopped with an error");
        ExitCode::FAILURE
    })
}

fn main() -> ExitCode {
    init_tracing();
    // Two workers are plenty for a loopback API; blocking threads (SQLite, file
    // reads) are spawned on demand and exit after 10 s idle, keeping idle RSS low.
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(16)
        .thread_keep_alive(Duration::from_secs(10))
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(err) => {
            eprintln!("intentd: cannot start the async runtime: {err}");
            return ExitCode::FAILURE;
        }
    };
    let code = match runtime.block_on(run()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(code) => code,
    };
    // A pending desktop-notification wait must not hold up exit.
    runtime.shutdown_timeout(Duration::from_secs(3));
    code
}
