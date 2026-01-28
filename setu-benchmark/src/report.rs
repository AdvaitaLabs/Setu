//! Report generation and output formatting

use crate::metrics::BenchmarkSummary;
use tracing::info;

/// Print benchmark report
pub fn print_report(summary: &BenchmarkSummary) {
    info!("╔══════════════════════════════════════════════════════════╗");
    info!("║                    BENCHMARK RESULTS                     ║");
    info!("╚══════════════════════════════════════════════════════════╝");
    info!("");
    
    // Overview
    info!("┌─────────────────────────────────────────────────────────┐");
    info!("│ OVERVIEW                                                │");
    info!("├─────────────────────────────────────────────────────────┤");
    info!("│ Total Requests:     {:>10}                          │", summary.total_requests);
    info!("│ Successful:         {:>10}                          │", summary.success_count);
    info!("│ Failed:             {:>10}                          │", summary.failure_count);
    info!("│ Timeout:            {:>10}                          │", summary.timeout_count);
    info!("│ Duration:           {:>10.2}s                         │", summary.elapsed_ms as f64 / 1000.0);
    info!("└─────────────────────────────────────────────────────────┘");
    info!("");

    // Performance
    info!("┌─────────────────────────────────────────────────────────┐");
    info!("│ PERFORMANCE                                             │");
    info!("├─────────────────────────────────────────────────────────┤");
    info!("│ ★ TPS (Successful): {:>10.2}                         │", summary.tps);
    info!("│ Success Rate:       {:>10.2}%                        │", summary.success_rate);
    info!("└─────────────────────────────────────────────────────────┘");
    info!("");

    // Latency
    info!("┌─────────────────────────────────────────────────────────┐");
    info!("│ LATENCY (milliseconds)                                  │");
    info!("├─────────────────────────────────────────────────────────┤");
    info!("│ Min:                {:>10.2} ms                       │", summary.latency.min_us as f64 / 1000.0);
    info!("│ Max:                {:>10.2} ms                       │", summary.latency.max_us as f64 / 1000.0);
    info!("│ Mean:               {:>10.2} ms                       │", summary.latency.mean_us / 1000.0);
    info!("│ P50 (Median):       {:>10.2} ms                       │", summary.latency.p50_us as f64 / 1000.0);
    info!("│ P90:                {:>10.2} ms                       │", summary.latency.p90_us as f64 / 1000.0);
    info!("│ P95:                {:>10.2} ms                       │", summary.latency.p95_us as f64 / 1000.0);
    info!("│ P99:                {:>10.2} ms                       │", summary.latency.p99_us as f64 / 1000.0);
    info!("│ P99.9:              {:>10.2} ms                       │", summary.latency.p999_us as f64 / 1000.0);
    info!("└─────────────────────────────────────────────────────────┘");
    info!("");

    // Summary line
    let status_emoji = if summary.success_rate > 99.0 {
        "✅"
    } else if summary.success_rate > 95.0 {
        "⚠️"
    } else {
        "❌"
    };

    info!(
        "{} Final TPS: {:.2} | Success Rate: {:.2}% | P99 Latency: {:.2}ms",
        status_emoji,
        summary.tps,
        summary.success_rate,
        summary.latency.p99_us as f64 / 1000.0
    );
}

/// Generate JSON report
#[allow(dead_code)]
pub fn json_report(summary: &BenchmarkSummary) -> String {
    serde_json::json!({
        "total_requests": summary.total_requests,
        "success_count": summary.success_count,
        "failure_count": summary.failure_count,
        "timeout_count": summary.timeout_count,
        "elapsed_ms": summary.elapsed_ms,
        "tps": summary.tps,
        "success_rate": summary.success_rate,
        "latency": {
            "min_ms": summary.latency.min_us as f64 / 1000.0,
            "max_ms": summary.latency.max_us as f64 / 1000.0,
            "mean_ms": summary.latency.mean_us / 1000.0,
            "p50_ms": summary.latency.p50_us as f64 / 1000.0,
            "p90_ms": summary.latency.p90_us as f64 / 1000.0,
            "p95_ms": summary.latency.p95_us as f64 / 1000.0,
            "p99_ms": summary.latency.p99_us as f64 / 1000.0,
            "p999_ms": summary.latency.p999_us as f64 / 1000.0,
        }
    }).to_string()
}

/// Generate CSV header
#[allow(dead_code)]
pub fn csv_header() -> &'static str {
    "timestamp,total_requests,success_count,failure_count,timeout_count,elapsed_ms,tps,success_rate,latency_min_ms,latency_max_ms,latency_mean_ms,latency_p50_ms,latency_p90_ms,latency_p95_ms,latency_p99_ms,latency_p999_ms"
}

/// Generate CSV row
#[allow(dead_code)]
pub fn csv_row(summary: &BenchmarkSummary) -> String {
    format!(
        "{},{},{},{},{},{},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2}",
        chrono::Utc::now().to_rfc3339(),
        summary.total_requests,
        summary.success_count,
        summary.failure_count,
        summary.timeout_count,
        summary.elapsed_ms,
        summary.tps,
        summary.success_rate,
        summary.latency.min_us as f64 / 1000.0,
        summary.latency.max_us as f64 / 1000.0,
        summary.latency.mean_us / 1000.0,
        summary.latency.p50_us as f64 / 1000.0,
        summary.latency.p90_us as f64 / 1000.0,
        summary.latency.p95_us as f64 / 1000.0,
        summary.latency.p99_us as f64 / 1000.0,
        summary.latency.p999_us as f64 / 1000.0,
    )
}
