use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use datafusion::datasource::file_format::parquet::ParquetFormat;
use datafusion::datasource::listing::{
    ListingOptions, ListingTable, ListingTableConfig, ListingTableUrl,
};
use datafusion::prelude::*;
use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;
use rb_shared::arrow_schema::{combat_event_schema, combat_events_to_record_batch};
use rb_shared::events::CombatEvent;
use rb_shared::proto::telemetry::telemetry_service_server::{
    TelemetryService, TelemetryServiceServer,
};
use rb_shared::proto::telemetry::{
    CombatEventProto, QueryRequest as GrpcQueryRequest, QueryResponse as GrpcQueryResponse,
    SendEventsRequest, SendEventsResponse,
};
use serde::Deserialize;
use tokio::sync::Mutex;
use tonic::{Request, Response, Status};
use tower_http::cors::CorsLayer;

struct AppState {
    buffer: Mutex<Vec<CombatEvent>>,
    data_dir: PathBuf,
    session_ctx: SessionContext,
}

#[derive(Deserialize)]
struct QueryRequest {
    sql: String,
}

// === gRPC Service Implementation ===

struct TelemetryServiceImpl {
    state: Arc<AppState>,
}

#[tonic::async_trait]
impl TelemetryService for TelemetryServiceImpl {
    async fn send_events(
        &self,
        request: Request<SendEventsRequest>,
    ) -> Result<Response<SendEventsResponse>, Status> {
        let proto_events = request.into_inner().events;
        let events: Vec<CombatEvent> = proto_events.into_iter().map(Into::into).collect();
        let count = events.len() as u32;

        let should_flush;
        {
            let mut buffer = self.state.buffer.lock().await;
            buffer.extend(events);
            tracing::info!(
                "gRPC: ingested {} events (buffer: {})",
                count,
                buffer.len()
            );
            should_flush = buffer.len() >= 64;
        }

        if should_flush {
            if let Err(e) = flush_buffer(&self.state).await {
                tracing::error!("flush after gRPC ingest failed: {}", e);
            }
        }

        Ok(Response::new(SendEventsResponse {
            events_received: count,
        }))
    }

    async fn query(
        &self,
        request: Request<GrpcQueryRequest>,
    ) -> Result<Response<GrpcQueryResponse>, Status> {
        let sql = request.into_inner().sql;

        // Flush pending events first
        if let Err(e) = flush_buffer(&self.state).await {
            tracing::warn!("pre-query flush failed: {}", e);
        }

        let df = self
            .state
            .session_ctx
            .sql(&sql)
            .await
            .map_err(|e| Status::invalid_argument(format!("SQL error: {}", e)))?;

        let batches = df
            .collect()
            .await
            .map_err(|e| Status::internal(format!("Execution error: {}", e)))?;

        let buf = Vec::new();
        let mut writer = arrow_json::ArrayWriter::new(buf);
        for batch in &batches {
            writer
                .write(batch)
                .map_err(|e| Status::internal(format!("JSON serialization error: {}", e)))?;
        }
        writer
            .finish()
            .map_err(|e| Status::internal(format!("JSON finish error: {}", e)))?;
        let json_bytes = writer.into_inner();
        let json_str = String::from_utf8(json_bytes)
            .map_err(|e| Status::internal(format!("UTF-8 error: {}", e)))?;

        Ok(Response::new(GrpcQueryResponse {
            json_rows: json_str,
        }))
    }
}

// === Main ===

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let data_dir = PathBuf::from("data");
    std::fs::create_dir_all(&data_dir).expect("failed to create data directory");

    let session_ctx = SessionContext::new();
    register_events_table(&session_ctx, &data_dir).await;

    let state = Arc::new(AppState {
        buffer: Mutex::new(Vec::new()),
        data_dir,
        session_ctx,
    });

    // Spawn background flush task
    let bg_state = Arc::clone(&state);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        loop {
            interval.tick().await;
            if let Err(e) = flush_buffer(&bg_state).await {
                tracing::error!("background flush failed: {}", e);
            }
        }
    });

    // Spawn gRPC server on port 50051
    let grpc_state = Arc::clone(&state);
    tokio::spawn(async move {
        let service = TelemetryServiceImpl {
            state: grpc_state,
        };
        tracing::info!("gRPC server listening on port 50051");
        tonic::transport::Server::builder()
            .add_service(TelemetryServiceServer::new(service))
            .serve("0.0.0.0:50051".parse().unwrap())
            .await
            .unwrap();
    });

    // REST server on port 3001 (health check + query + legacy event ingest)
    let app = Router::new()
        .route("/health", get(health))
        .route("/events", post(ingest_events))
        .route("/query", get(handle_query))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3001")
        .await
        .unwrap();
    tracing::info!("REST server listening on port 3001");
    axum::serve(listener, app).await.unwrap();
}

// === REST Handlers (kept for debugging/tooling) ===

async fn health() -> StatusCode {
    StatusCode::OK
}

async fn ingest_events(
    State(state): State<Arc<AppState>>,
    Json(batch): Json<Vec<CombatEvent>>,
) -> StatusCode {
    let count = batch.len();
    let should_flush;
    {
        let mut buffer = state.buffer.lock().await;
        buffer.extend(batch);
        tracing::info!("REST: ingested {} events (buffer: {})", count, buffer.len());
        should_flush = buffer.len() >= 64;
    }
    if should_flush {
        if let Err(e) = flush_buffer(&state).await {
            tracing::error!("flush after ingest failed: {}", e);
        }
    }
    StatusCode::ACCEPTED
}

async fn handle_query(
    State(state): State<Arc<AppState>>,
    Json(req): Json<QueryRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    if let Err(e) = flush_buffer(&state).await {
        tracing::warn!("pre-query flush failed: {}", e);
    }

    let df = state
        .session_ctx
        .sql(&req.sql)
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("SQL error: {}", e)))?;

    let batches = df.collect().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Execution error: {}", e),
        )
    })?;

    let buf = Vec::new();
    let mut writer = arrow_json::ArrayWriter::new(buf);
    for batch in &batches {
        writer.write(batch).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("JSON serialization error: {}", e),
            )
        })?;
    }
    writer.finish().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("JSON finish error: {}", e),
        )
    })?;
    let json_bytes = writer.into_inner();

    let rows: serde_json::Value =
        serde_json::from_slice(&json_bytes).unwrap_or_else(|_| serde_json::Value::Array(vec![]));

    Ok(Json(rows))
}

// === Shared Logic ===

async fn flush_buffer(state: &AppState) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let events: Vec<CombatEvent>;
    {
        let mut buffer = state.buffer.lock().await;
        if buffer.is_empty() {
            return Ok(());
        }
        events = std::mem::take(&mut *buffer);
    }

    let count = events.len();
    let record_batch = combat_events_to_record_batch(&events)?;

    let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S_%3f");
    let filename = format!("events_{}.parquet", timestamp);
    let filepath = state.data_dir.join(&filename);

    let file = std::fs::File::create(&filepath)?;
    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .build();
    let mut writer = ArrowWriter::try_new(file, record_batch.schema(), Some(props))?;
    writer.write(&record_batch)?;
    writer.close()?;

    tracing::info!("flushed {} events to {}", count, filepath.display());

    Ok(())
}

async fn register_events_table(ctx: &SessionContext, data_dir: &PathBuf) {
    let abs_data_dir = std::fs::canonicalize(data_dir)
        .unwrap_or_else(|_| std::env::current_dir().unwrap().join(data_dir));

    let table_path = match ListingTableUrl::parse(abs_data_dir.to_string_lossy().as_ref()) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("failed to parse data dir as listing table URL: {}", e);
            return;
        }
    };

    let file_format = ParquetFormat::new();
    let listing_options =
        ListingOptions::new(Arc::new(file_format)).with_file_extension(".parquet");

    let arrow_schema = combat_event_schema();
    let schema = Arc::new(arrow_schema);

    let config = ListingTableConfig::new(table_path)
        .with_listing_options(listing_options)
        .with_schema(schema);

    match ListingTable::try_new(config) {
        Ok(table) => {
            if let Err(e) = ctx.register_table("events", Arc::new(table)) {
                tracing::warn!("failed to register events listing table: {}", e);
            } else {
                tracing::info!(
                    "registered events listing table from {}",
                    abs_data_dir.display()
                );
            }
        }
        Err(e) => {
            tracing::warn!("failed to create events listing table: {}", e);
        }
    }
}
