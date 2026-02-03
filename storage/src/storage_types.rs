//! Storage-specific types
//!
//! This module contains types used across storage implementations.
//! Moving them here prevents circular dependencies between implementation
//! modules and backend trait modules.

use setu_types::EventId;

/// Result of a batch store operation
/// 
/// This type provides detailed information about what happened during
/// a batch store operation, allowing callers to handle various scenarios:
/// - Complete success (all events stored)
/// - Partial success (some duplicates skipped)
/// - Partial failure (some events failed to store)
#[derive(Debug, Default, Clone)]
pub struct BatchStoreResult {
    /// Number of events successfully stored
    pub stored: usize,
    /// Number of events skipped (duplicates - non-critical)
    pub skipped: usize,
    /// Number of events that failed to store (critical errors)
    pub failed: usize,
    /// IDs of skipped events (duplicates)
    pub skipped_ids: Vec<EventId>,
    /// IDs and errors of failed events
    pub failed_errors: Vec<(EventId, String)>,
}

impl BatchStoreResult {
    /// Create a new empty result
    pub fn new() -> Self {
        Self::default()
    }
    
    /// Check if there were any critical failures
    pub fn has_critical_failures(&self) -> bool {
        self.failed > 0
    }
    
    /// Total events processed
    pub fn total(&self) -> usize {
        self.stored + self.skipped + self.failed
    }
    
    /// Check if operation was fully successful (no failures, no skips)
    pub fn is_complete_success(&self) -> bool {
        self.failed == 0 && self.skipped == 0
    }
    
    /// Check if operation was successful (no failures, skips are OK)
    pub fn is_success(&self) -> bool {
        self.failed == 0
    }
    
    /// Merge another result into this one
    pub fn merge(&mut self, other: BatchStoreResult) {
        self.stored += other.stored;
        self.skipped += other.skipped;
        self.failed += other.failed;
        self.skipped_ids.extend(other.skipped_ids);
        self.failed_errors.extend(other.failed_errors);
    }
}

impl std::fmt::Display for BatchStoreResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "BatchStoreResult {{ stored: {}, skipped: {}, failed: {}, total: {} }}",
            self.stored, self.skipped, self.failed, self.total()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_batch_store_result_default() {
        let result = BatchStoreResult::new();
        assert_eq!(result.stored, 0);
        assert_eq!(result.skipped, 0);
        assert_eq!(result.failed, 0);
        assert!(result.is_success());
        assert!(result.is_complete_success());
    }

    #[test]
    fn test_batch_store_result_with_skips() {
        let mut result = BatchStoreResult::new();
        result.stored = 5;
        result.skipped = 2;
        result.skipped_ids = vec!["id1".to_string(), "id2".to_string()];
        
        assert_eq!(result.total(), 7);
        assert!(result.is_success());
        assert!(!result.is_complete_success());
        assert!(!result.has_critical_failures());
    }

    #[test]
    fn test_batch_store_result_with_failures() {
        let mut result = BatchStoreResult::new();
        result.stored = 3;
        result.failed = 1;
        result.failed_errors = vec![("id1".to_string(), "some error".to_string())];
        
        assert_eq!(result.total(), 4);
        assert!(!result.is_success());
        assert!(result.has_critical_failures());
    }

    #[test]
    fn test_batch_store_result_merge() {
        let mut result1 = BatchStoreResult::new();
        result1.stored = 3;
        result1.skipped = 1;
        
        let mut result2 = BatchStoreResult::new();
        result2.stored = 2;
        result2.failed = 1;
        
        result1.merge(result2);
        
        assert_eq!(result1.stored, 5);
        assert_eq!(result1.skipped, 1);
        assert_eq!(result1.failed, 1);
        assert_eq!(result1.total(), 7);
    }
}
