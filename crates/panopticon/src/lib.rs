pub mod replay;

use opentelemetry::global;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::trace::{self, Sampler};
use savant_ipc::SwarmSharedContext;
use std::collections::HashMap;
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, Registry};

/// Initializes the Panopticon distributed telemetry engine.
///
/// Sets up an OpenTelemetry pipeline with non-blocking batch exporting
/// and registers the global tracer provider.
pub fn init_panopticon(service_name: &str, otlp_endpoint: &str) -> anyhow::Result<()> {
    // Set global propagator for W3C TraceContext
    global::set_text_map_propagator(TraceContextPropagator::new());

    let tracer = opentelemetry_otlp::new_pipeline()
        .tracing()
        .with_exporter(
            opentelemetry_otlp::new_exporter()
                .tonic()
                .with_endpoint(otlp_endpoint),
        )
        .with_trace_config(
            trace::config()
                .with_sampler(Sampler::AlwaysOn)
                .with_resource(opentelemetry_sdk::Resource::new(vec![
                    opentelemetry::KeyValue::new("service.name", service_name.to_string()),
                ])),
        )
        .install_batch(opentelemetry_sdk::runtime::Tokio)?;

    let telemetry = OpenTelemetryLayer::new(tracer);

    match Registry::default()
        .with(tracing_subscriber::EnvFilter::from_default_env())
        .with(tracing_subscriber::fmt::layer())
        .with(telemetry)
        .try_init()
    {
        Ok(_) => {}
        Err(e) if e.to_string().contains("already been set") => {
            // Already initialized by another subsystem, this is fine
            tracing::debug!("Tracing subscriber already initialized, skipping: {}", e);
        }
        Err(e) => {
            eprintln!("Warning: Failed to initialize tracing: {}", e);
        }
    }

    Ok(())
}

/// Injects the current tracing context into a SwarmSharedContext for IPC propagation.
pub fn inject_trace_context(ctx: &mut SwarmSharedContext) {
    let context = global::get_text_map_propagator(|propagator| {
        let mut fields = HashMap::new();
        propagator.inject_context(&opentelemetry::Context::current(), &mut fields);
        fields
    });

    // Extract traceparent from the injected fields (W3C standard)
    if let Some(traceparent) = context.get("traceparent") {
        // Format: 00-traceid-spanid-flags
        let parts: Vec<&str> = traceparent.split('-').collect();
        if parts.len() >= 3 {
            if let Ok(trace_id) = hex::decode(parts[1]) {
                if trace_id.len() == 16 {
                    ctx.trace_id.copy_from_slice(&trace_id);
                }
            }
            if let Ok(span_id) = hex::decode(parts[2]) {
                if span_id.len() == 8 {
                    ctx.span_id.copy_from_slice(&span_id);
                }
            }
        }
    }
}

/// Extracts tracing context from a SwarmSharedContext to resume a trace in the current process.
pub fn extract_trace_context(ctx: &SwarmSharedContext) -> opentelemetry::Context {
    let traceparent = format!(
        "00-{}-{}-01",
        hex::encode(ctx.trace_id),
        hex::encode(ctx.span_id)
    );

    let mut carrier = HashMap::new();
    carrier.insert("traceparent".to_string(), traceparent);

    global::get_text_map_propagator(|propagator| propagator.extract(&carrier))
}
