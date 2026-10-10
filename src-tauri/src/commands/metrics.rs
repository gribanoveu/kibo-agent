//! The Usage heatmap's counters, read for Settings.

use crate::domain::metrics::{DailyMetric, Metric};
use crate::infra::daily_metrics;

#[tauri::command]
pub fn daily_metrics(metrics: Vec<Metric>) -> Result<Vec<DailyMetric>, String> {
    daily_metrics::read(&metrics).map_err(|e| e.to_string())
}
