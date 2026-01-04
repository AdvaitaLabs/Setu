//! Quick check module for fast validation

use core_types::Transfer;
use thiserror::Error;
use tracing::debug;

/// Quick check errors
#[derive(Debug, Error)]
pub enum QuickCheckError {
    #[error("Transfer ID is empty")]
    EmptyTransferId,
    
    #[error("Invalid sender: {0}")]
    InvalidSender(String),
    
    #[error("Invalid recipient: {0}")]
    InvalidRecipient(String),
    
    #[error("Invalid amount: {0}")]
    InvalidAmount(i128),
    
    #[error("Empty resources")]
    EmptyResources,
    
    #[error("VLC is invalid")]
    InvalidVLC,
    
    #[error("Check timeout")]
    Timeout,
}

/// Quick checker for fast validation
pub struct QuickChecker {
    timeout_ms: u64,
}

impl QuickChecker {
    pub fn new(timeout_ms: u64) -> Self {
        Self { timeout_ms }
    }
    
    /// Perform quick check on transfer
    pub async fn check(&self, transfer: &Transfer) -> Result<(), QuickCheckError> {
        debug!(
            transfer_id = %transfer.id,
            "Starting quick check"
        );
        
        // Check 1: Transfer ID must not be empty
        if transfer.id.is_empty() {
            return Err(QuickCheckError::EmptyTransferId);
        }
        
        // Check 2: Sender must not be empty
        if transfer.from.is_empty() {
            return Err(QuickCheckError::InvalidSender(
                "Sender cannot be empty".to_string()
            ));
        }
        
        // Check 3: Recipient must not be empty (for most transfer types)
        if transfer.to.is_empty() {
            return Err(QuickCheckError::InvalidRecipient(
                "Recipient cannot be empty".to_string()
            ));
        }
        
        // Check 4: Amount must be positive
        if transfer.amount <= 0 {
            return Err(QuickCheckError::InvalidAmount(transfer.amount));
        }
        
        // Check 5: Resources must not be empty
        if transfer.resources.is_empty() {
            return Err(QuickCheckError::EmptyResources);
        }
        
        // Check 6: VLC must have at least one entry
        if transfer.vlc.entries.is_empty() {
            return Err(QuickCheckError::InvalidVLC);
        }
        
        // Check 7: Sender and recipient should be different
        if transfer.from == transfer.to {
            return Err(QuickCheckError::InvalidRecipient(
                "Sender and recipient cannot be the same".to_string()
            ));
        }
        
        debug!(
            transfer_id = %transfer.id,
            "Quick check passed"
        );
        
        Ok(())
    }
    
    /// Check if transfer format is valid (basic structure)
    pub fn check_format(&self, transfer: &Transfer) -> Result<(), QuickCheckError> {
        // Basic format checks without async
        if transfer.id.is_empty() {
            return Err(QuickCheckError::EmptyTransferId);
        }
        
        if transfer.from.is_empty() {
            return Err(QuickCheckError::InvalidSender("Empty sender".to_string()));
        }
        
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_types::{TransferType, Vlc};
    
    fn create_valid_transfer() -> Transfer {
        let mut vlc = Vlc::new();
        vlc.entries.insert("node1".to_string(), 1);
        
        Transfer {
            id: "transfer_1".to_string(),
            from: "alice".to_string(),
            to: "bob".to_string(),
            amount: 100,
            transfer_type: TransferType::FluxTransfer,
            resources: vec!["alice".to_string()],
            vlc,
            power: 10,
        }
    }
    
    #[tokio::test]
    async fn test_valid_transfer() {
        let checker = QuickChecker::new(100);
        let transfer = create_valid_transfer();
        
        let result = checker.check(&transfer).await;
        assert!(result.is_ok());
    }
    
    #[tokio::test]
    async fn test_empty_transfer_id() {
        let checker = QuickChecker::new(100);
        let mut transfer = create_valid_transfer();
        transfer.id = "".to_string();
        
        let result = checker.check(&transfer).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), QuickCheckError::EmptyTransferId));
    }
    
    #[tokio::test]
    async fn test_invalid_amount() {
        let checker = QuickChecker::new(100);
        let mut transfer = create_valid_transfer();
        transfer.amount = 0;
        
        let result = checker.check(&transfer).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), QuickCheckError::InvalidAmount(_)));
    }
    
    #[tokio::test]
    async fn test_same_sender_recipient() {
        let checker = QuickChecker::new(100);
        let mut transfer = create_valid_transfer();
        transfer.to = transfer.from.clone();
        
        let result = checker.check(&transfer).await;
        assert!(result.is_err());
    }
}







