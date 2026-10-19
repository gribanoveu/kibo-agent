//! The Usage heatmap's counters, read for Settings.

use crate::domain::metrics::{DailyMetric, Metric};
use crate::infra::daily_metrics;

use super::blocking;

#[tauri::command]
pub async fn daily_metrics(metrics: Vec<Metric>) -> Result<Vec<DailyMetric>, String> {
    blocking(move || daily_metrics::read(&metrics)).await
}
