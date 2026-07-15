use std::net::SocketAddr;
use tracing::info;

pub async fn run(port: u16) -> anyhow::Result<()> {
    let state = ox_server::AppState {
        scope_cache: ox_core::ScopeCache::new(),
        dataflow_cache: ox_server::DataflowCache::new(),
    };
    let app = ox_server::router(state);
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    info!("ox-codes listening on {addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
