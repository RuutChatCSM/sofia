use pretty_assertions::assert_eq;
use sofia_utils_absolute_path::test_support::PathExt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use uuid::Uuid;

use super::*;

#[tokio::test]
async fn sqlite_sink_filters_noisy_targets_without_dropping_useful_diagnostics() {
    let sofia_home =
        std::env::temp_dir().join(format!("sofia-state-log-db-filter-{}", Uuid::new_v4()));
    let _cleanup = scopeguard::guard(sofia_home.clone(), |sofia_home| {
        let _ = std::fs::remove_dir_all(sofia_home);
    });
    let runtime = StateRuntime::init(
        crate::SqliteConfig::new_for_testing(sofia_home.as_path().abs()),
        "test-provider".to_string(),
    )
    .await
    .expect("initialize runtime");
    let layer = start(runtime.clone());

    let guard = tracing_subscriber::registry()
        .with(layer.clone().with_filter(default_filter()))
        .set_default();

    tracing::trace!(target: "opentelemetry_sdk", "dropped-trace");
    tracing::debug!(target: "opentelemetry_sdk", "dropped-debug");
    tracing::info!(target: "opentelemetry_sdk", "retained-info");
    tracing::warn!(target: "sqlx::query", "dropped-slow-query-warning");
    tracing::warn!(target: "sqlx::pool::acquire", "dropped-slow-acquire-warning");
    tracing::warn!(target: "sqlx::other", "dropped-sqlx-warning");
    tracing::info!(target: "sqlx_application", "retained-unrelated-target");
    tracing::debug!(target: "opentelemetry-otlp", "dropped-otlp-export");
    tracing::error!(target: "opentelemetry-http", "dropped-http-export");
    tracing::trace!(target: "h2::proto::streams", "dropped-http2-trace");
    tracing::debug!(target: "tonic::transport::channel", "dropped-grpc-transport-debug");
    tracing::debug!(target: "tower::buffer::worker", "dropped-grpc-buffer-debug");
    tracing::warn!(
        target: "h2::proto::ping_pong",
        "recv PING ack that we never sent: {:?}",
        "synthetic-ack"
    );
    tracing::warn!(target: "h2::proto::ping_pong", "retained-http2-ping-warning");
    tracing::warn!(target: "h2::proto::streams", "retained-http2-warning");
    tracing::debug!(target: "rmcp::transport", "dropped-rmcp-debug");
    tracing::info!(target: "rmcp::transport", "retained-rmcp-info");
    tracing::debug!(
        target: "sofia_rmcp_client::oauth",
        "dropped-sofia-rmcp-client-debug"
    );
    tracing::info!(
        target: "sofia_rmcp_client::oauth",
        "retained-sofia-rmcp-client-info"
    );
    tracing::trace!(target: "sofia_http_client::transport", "dropped-request-body");
    tracing::debug!(target: "sofia_http_client::transport", "retained-request-diagnostic");
    tracing::trace!(target: "sofia_api::sse", "dropped-sse-parent");
    tracing::trace!(target: "sofia_api::sse::responses", "dropped-sse-payload");
    tracing::debug!(target: "sofia_api::sse::responses", "retained-sse-diagnostic");
    tracing::trace!(target: "sofia_state", "retained-trace");
    tracing::trace!(
        target: "sofia_tui::streaming::controller",
        "dropped-controller-trace"
    );
    tracing::debug!(
        target: "sofia_tui::streaming::controller",
        "retained-controller-debug"
    );
    tracing::trace!(
        target: "sofia_tui::streaming::table_holdback",
        "dropped-table-holdback-trace"
    );
    tracing::debug!(
        target: "sofia_tui::streaming::table_holdback",
        "retained-table-holdback-debug"
    );
    tracing::trace!(
        target: "sofia_tui::streaming::commit_tick",
        "retained-commit-tick-trace"
    );
    tracing::trace!(
        target: "sofia_api::responses_websocket_timing",
        payload = "complete timing payload",
        "dropped-websocket-timing"
    );

    layer.flush().await;
    drop(guard);

    let logs = runtime
        .query_logs(&crate::LogQuery::default())
        .await
        .expect("query logs after flush");
    assert_eq!(
        logs.iter()
            .map(|row| (
                row.level.as_str(),
                row.target.as_str(),
                row.message.as_deref()
            ))
            .collect::<Vec<_>>(),
        vec![
            ("INFO", "opentelemetry_sdk", Some("retained-info")),
            (
                "INFO",
                "sqlx_application",
                Some("retained-unrelated-target")
            ),
            (
                "WARN",
                "h2::proto::ping_pong",
                Some("retained-http2-ping-warning")
            ),
            ("WARN", "h2::proto::streams", Some("retained-http2-warning")),
            ("INFO", "rmcp::transport", Some("retained-rmcp-info")),
            (
                "INFO",
                "sofia_rmcp_client::oauth",
                Some("retained-sofia-rmcp-client-info")
            ),
            (
                "DEBUG",
                "sofia_http_client::transport",
                Some("retained-request-diagnostic")
            ),
            (
                "DEBUG",
                "sofia_api::sse::responses",
                Some("retained-sse-diagnostic")
            ),
            ("TRACE", "sofia_state", Some("retained-trace")),
            (
                "DEBUG",
                "sofia_tui::streaming::controller",
                Some("retained-controller-debug"),
            ),
            (
                "DEBUG",
                "sofia_tui::streaming::table_holdback",
                Some("retained-table-holdback-debug"),
            ),
            (
                "TRACE",
                "sofia_tui::streaming::commit_tick",
                Some("retained-commit-tick-trace"),
            ),
        ]
    );
}
