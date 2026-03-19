use chrono::{Datelike, Utc};

use super::router::AppUsageSummary;
use super::types::UsageRecord;

/// Tracks LLM usage and budget per app and provider.
/// In-memory for now; could be backed by SQLite later.
pub struct BudgetTracker {
    records: Vec<UsageRecord>,
}

impl BudgetTracker {
    pub fn new() -> Self {
        Self {
            records: Vec::new(),
        }
    }

    pub fn record_usage(&mut self, record: UsageRecord) {
        self.records.push(record);
    }

    /// Get the total cost for an app in the current month.
    pub fn monthly_cost_for_app(&self, app_id: &str) -> f64 {
        let now = Utc::now();
        self.records
            .iter()
            .filter(|r| {
                r.app_id == app_id
                    && r.timestamp.year() == now.year()
                    && r.timestamp.month() == now.month()
            })
            .map(|r| r.cost_usd)
            .sum()
    }

    /// Get daily request count and cost for a provider.
    pub fn daily_stats_for_provider(&self, provider: &str) -> (u64, f64) {
        let today = Utc::now().date_naive();
        let (count, cost) = self
            .records
            .iter()
            .filter(|r| r.provider == provider && r.timestamp.date_naive() == today)
            .fold((0u64, 0.0f64), |(c, cost), r| (c + 1, cost + r.cost_usd));
        (count, cost)
    }

    /// Get usage summary for a specific app.
    pub fn summary_for_app(&self, app_id: &str) -> AppUsageSummary {
        let now = Utc::now();
        let app_records: Vec<_> = self.records.iter().filter(|r| r.app_id == app_id).collect();

        let total_requests = app_records.len() as u64;
        let total_input_tokens: u64 = app_records.iter().map(|r| r.input_tokens as u64).sum();
        let total_output_tokens: u64 = app_records.iter().map(|r| r.output_tokens as u64).sum();
        let total_cost_usd: f64 = app_records.iter().map(|r| r.cost_usd).sum();

        let month_cost_usd: f64 = app_records
            .iter()
            .filter(|r| r.timestamp.year() == now.year() && r.timestamp.month() == now.month())
            .map(|r| r.cost_usd)
            .sum();

        AppUsageSummary {
            app_id: app_id.to_string(),
            total_requests,
            total_input_tokens,
            total_output_tokens,
            total_cost_usd,
            month_cost_usd,
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_record(app_id: &str, provider: &str, cost: f64) -> UsageRecord {
        UsageRecord {
            app_id: app_id.to_string(),
            provider: provider.to_string(),
            model: "test-model".to_string(),
            input_tokens: 100,
            output_tokens: 50,
            cost_usd: cost,
            timestamp: Utc::now(),
            request_id: "req-1".to_string(),
            duration_ms: 500,
        }
    }

    #[test]
    fn test_monthly_cost() {
        let mut tracker = BudgetTracker::new();
        tracker.record_usage(make_record("app1", "ollama", 0.01));
        tracker.record_usage(make_record("app1", "ollama", 0.02));
        tracker.record_usage(make_record("app2", "ollama", 0.05));

        assert!((tracker.monthly_cost_for_app("app1") - 0.03).abs() < 0.001);
        assert!((tracker.monthly_cost_for_app("app2") - 0.05).abs() < 0.001);
        assert!((tracker.monthly_cost_for_app("app3") - 0.0).abs() < 0.001);
    }

    #[test]
    fn test_daily_stats() {
        let mut tracker = BudgetTracker::new();
        tracker.record_usage(make_record("app1", "ollama", 0.01));
        tracker.record_usage(make_record("app1", "ollama", 0.02));
        tracker.record_usage(make_record("app1", "anthropic", 0.10));

        let (count, cost) = tracker.daily_stats_for_provider("ollama");
        assert_eq!(count, 2);
        assert!((cost - 0.03).abs() < 0.001);
    }

    #[test]
    fn test_summary() {
        let mut tracker = BudgetTracker::new();
        tracker.record_usage(make_record("app1", "ollama", 0.01));
        tracker.record_usage(make_record("app1", "anthropic", 0.10));

        let summary = tracker.summary_for_app("app1");
        assert_eq!(summary.total_requests, 2);
        assert_eq!(summary.total_input_tokens, 200);
        assert_eq!(summary.total_output_tokens, 100);
        assert!((summary.total_cost_usd - 0.11).abs() < 0.001);
    }

}
