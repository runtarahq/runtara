// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Local-only OTLP receiver for e2e/test_pipeline_analytics.sh.
use opentelemetry_proto::tonic::collector::{
    logs::v1::{
        ExportLogsServiceRequest, ExportLogsServiceResponse,
        logs_service_server::{LogsService, LogsServiceServer},
    },
    metrics::v1::{
        ExportMetricsServiceRequest, ExportMetricsServiceResponse,
        metrics_service_server::{MetricsService, MetricsServiceServer},
    },
    trace::v1::{
        ExportTraceServiceRequest, ExportTraceServiceResponse,
        trace_service_server::{TraceService, TraceServiceServer},
    },
};
use std::io::Write;
use std::sync::Mutex;
use tonic::{Request, Response, Status};

struct Receiver {
    output: Mutex<std::fs::File>,
}
#[tonic::async_trait]
impl MetricsService for Receiver {
    async fn export(
        &self,
        request: Request<ExportMetricsServiceRequest>,
    ) -> Result<Response<ExportMetricsServiceResponse>, Status> {
        let mut output = self.output.lock().unwrap();
        serde_json::to_writer(&mut *output, &request.into_inner())
            .map_err(|_| Status::internal("write metrics"))?;
        writeln!(output).map_err(|_| Status::internal("write newline"))?;
        output.flush().map_err(|_| Status::internal("flush"))?;
        Ok(Response::new(ExportMetricsServiceResponse::default()))
    }
}
struct Discard;
#[tonic::async_trait]
impl TraceService for Discard {
    async fn export(
        &self,
        _: Request<ExportTraceServiceRequest>,
    ) -> Result<Response<ExportTraceServiceResponse>, Status> {
        Ok(Response::new(ExportTraceServiceResponse::default()))
    }
}
#[tonic::async_trait]
impl LogsService for Discard {
    async fn export(
        &self,
        _: Request<ExportLogsServiceRequest>,
    ) -> Result<Response<ExportLogsServiceResponse>, Status> {
        Ok(Response::new(ExportLogsServiceResponse::default()))
    }
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let port: u16 = args.next().ok_or("port required")?.parse()?;
    let output = Mutex::new(std::fs::File::create(
        args.next().ok_or("output file required")?,
    )?);
    tonic::transport::Server::builder()
        .add_service(MetricsServiceServer::new(Receiver { output }))
        .add_service(TraceServiceServer::new(Discard))
        .add_service(LogsServiceServer::new(Discard))
        .serve(([127, 0, 0, 1], port).into())
        .await?;
    Ok(())
}
