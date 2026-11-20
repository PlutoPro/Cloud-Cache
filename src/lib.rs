pub mod auth;
pub mod config;
pub mod http;
pub mod identity;
pub mod store;
pub mod tcp;

use std::{
    future::IntoFuture,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

pub use config::Config;

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<store::Store>,
    pub verifier: Arc<auth::Verifier>,
    /// Flipped to false on shutdown so /readyz fails and the LB drains this node.
    pub ready: Arc<AtomicBool>,
    pub max_value_bytes: usize,
    pub tcp_idle_timeout: Duration,
    pub node_id: Arc<str>,
    /// Unix seconds; lets usage collectors detect counter resets after a restart.
    pub started_at: u64,
}

/// Bind both listeners and run until `shutdown` is cancelled.
pub async fn run(cfg: Config, shutdown: CancellationToken) -> anyhow::Result<()> {
    let http_l = TcpListener::bind(cfg.http_addr).await?;
    let tcp_l = TcpListener::bind(cfg.tcp_addr).await?;
    run_with_listeners(cfg, http_l, tcp_l, shutdown).await
}

/// Run with the claim mapping from configuration (`AUTH_PRESET` and overrides).
pub async fn run_with_listeners(
    cfg: Config,
    http_l: TcpListener,
    tcp_l: TcpListener,
    shutdown: CancellationToken,
) -> anyhow::Result<()> {
    let mapper = Arc::new(identity::ConfiguredMapper::new(cfg.mapping.clone()));
    run_with(cfg, mapper, http_l, tcp_l, shutdown).await
}

/// Run with a custom [`identity::ClaimMapper`], for providers configuration can't describe.
pub async fn run_with(
    cfg: Config,
    mapper: Arc<dyn identity::ClaimMapper>,
    http_l: TcpListener,
    tcp_l: TcpListener,
    shutdown: CancellationToken,
) -> anyhow::Result<()> {
    let verifier = Arc::new(auth::Verifier::new(&cfg, mapper)?);
    let state = AppState {
        store: Arc::new(store::Store::default()),
        verifier: verifier.clone(),
        ready: Arc::new(AtomicBool::new(true)),
        max_value_bytes: cfg.max_value_bytes,
        tcp_idle_timeout: cfg.tcp_idle_timeout,
        node_id: cfg.node_id.as_str().into(),
        started_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    };
    // Listeners close only after the drain window.
    let stop = CancellationToken::new();

    // JWKS loader: retry quickly until the provider answers, then refresh every 5 minutes.
    {
        let v = verifier.clone();
        let stop = stop.clone();
        tokio::spawn(async move {
            loop {
                let wait = match v.refresh(true).await {
                    Ok(n) => {
                        let url = v.jwks_url().await;
                        tracing::info!(keys = n, %url, "loaded signing keys");
                        Duration::from_secs(300)
                    }
                    Err(e) => {
                        let url = v.jwks_url().await;
                        tracing::warn!(error = %e, %url, "JWKS fetch failed; retrying");
                        Duration::from_secs(2)
                    }
                };
                tokio::select! { _ = stop.cancelled() => break, _ = tokio::time::sleep(wait) => {} }
            }
        });
    }

    // TTL sweeper.
    {
        let store = state.store.clone();
        let stop = stop.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(5));
            loop {
                tokio::select! { _ = stop.cancelled() => break, _ = tick.tick() => { store.sweep(); } }
            }
        });
    }

    // Graceful drain: fail readiness, give the LB time to notice, then close listeners.
    {
        let ready = state.ready.clone();
        let stop = stop.clone();
        let drain = cfg.drain;
        tokio::spawn(async move {
            shutdown.cancelled().await;
            ready.store(false, Ordering::Relaxed);
            tracing::info!(?drain, "shutdown requested; draining");
            tokio::time::sleep(drain).await;
            stop.cancel();
        });
    }

    tracing::info!(
        http = %http_l.local_addr()?,
        tcp = %tcp_l.local_addr()?,
        issuer = %cfg.issuer,
        mapping = ?identity::describe(&cfg.mapping),
        "cloud cache listening"
    );

    let http = axum::serve(http_l, http::router(state.clone()))
        .with_graceful_shutdown(stop.clone().cancelled_owned());
    let tcp = tcp::serve(tcp_l, state, stop);
    let (http_res, ()) = tokio::join!(http.into_future(), tcp);
    http_res?;
    tracing::info!("stopped");
    Ok(())
}
