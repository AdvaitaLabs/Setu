//! Pending queue for transfers awaiting routing

use core_types::Transfer;
use std::collections::{HashMap, VecDeque};
use thiserror::Error;
use tracing::{debug, warn};

/// Pending queue errors
#[derive(Debug, Error)]
pub enum PendingQueueError {
    #[error("Queue is full (max size: {0})")]
    QueueFull(usize),
    
    #[error("Transfer not found: {0}")]
    TransferNotFound(String),
    
    #[error("Duplicate transfer: {0}")]
    DuplicateTransfer(String),
}

/// Pending transfer with metadata
#[derive(Debug, Clone)]
pub struct PendingTransfer {
    pub transfer: Transfer,
    pub enqueued_at: u64,
    pub priority: u32,
}

impl PendingTransfer {
    pub fn new(transfer: Transfer) -> Self {
        let enqueued_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        
        // Calculate priority based on power score
        let priority = transfer.power as u32;
        
        Self {
            transfer,
            enqueued_at,
            priority,
        }
    }
}

/// Pending queue for transfers
pub struct PendingQueue {
    /// Maximum queue size
    max_size: usize,
    
    /// Queue of pending transfers (FIFO by default)
    queue: VecDeque<PendingTransfer>,
    
    /// Index for fast lookup by transfer ID
    index: HashMap<String, usize>,
}

impl PendingQueue {
    /// Create a new pending queue
    pub fn new(max_size: usize) -> Self {
        Self {
            max_size,
            queue: VecDeque::new(),
            index: HashMap::new(),
        }
    }
    
    /// Enqueue a transfer
    pub fn enqueue(&mut self, transfer: Transfer) -> Result<(), PendingQueueError> {
        // Check if queue is full
        if self.queue.len() >= self.max_size {
            warn!(
                queue_size = self.queue.len(),
                max_size = self.max_size,
                "Pending queue is full"
            );
            return Err(PendingQueueError::QueueFull(self.max_size));
        }
        
        // Check for duplicates
        if self.index.contains_key(&transfer.id) {
            return Err(PendingQueueError::DuplicateTransfer(transfer.id.clone()));
        }
        
        let transfer_id = transfer.id.clone();
        let pending = PendingTransfer::new(transfer);
        
        // Add to queue
        let position = self.queue.len();
        self.queue.push_back(pending);
        self.index.insert(transfer_id.clone(), position);
        
        debug!(
            transfer_id = %transfer_id,
            queue_size = self.queue.len(),
            "Transfer enqueued"
        );
        
        Ok(())
    }
    
    /// Dequeue a specific transfer by ID
    pub fn dequeue(&mut self, transfer_id: &str) -> Result<Transfer, PendingQueueError> {
        // Find the transfer
        let position = self.index.remove(transfer_id)
            .ok_or_else(|| PendingQueueError::TransferNotFound(transfer_id.to_string()))?;
        
        // Remove from queue
        let pending = self.queue.remove(position)
            .ok_or_else(|| PendingQueueError::TransferNotFound(transfer_id.to_string()))?;
        
        // Rebuild index (positions may have shifted)
        self.rebuild_index();
        
        debug!(
            transfer_id = %transfer_id,
            queue_size = self.queue.len(),
            "Transfer dequeued"
        );
        
        Ok(pending.transfer)
    }
    
    /// Dequeue the next transfer (FIFO)
    pub fn dequeue_next(&mut self) -> Option<Transfer> {
        if let Some(pending) = self.queue.pop_front() {
            self.index.remove(&pending.transfer.id);
            self.rebuild_index();
            
            debug!(
                transfer_id = %pending.transfer.id,
                queue_size = self.queue.len(),
                "Next transfer dequeued"
            );
            
            Some(pending.transfer)
        } else {
            None
        }
    }
    
    /// Peek at the next transfer without removing it
    pub fn peek_next(&self) -> Option<&Transfer> {
        self.queue.front().map(|p| &p.transfer)
    }
    
    /// Get queue size
    pub fn size(&self) -> usize {
        self.queue.len()
    }
    
    /// Check if queue is empty
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
    
    /// Check if queue is full
    pub fn is_full(&self) -> bool {
        self.queue.len() >= self.max_size
    }
    
    /// Get all pending transfer IDs
    pub fn pending_ids(&self) -> Vec<String> {
        self.queue.iter().map(|p| p.transfer.id.clone()).collect()
    }
    
    /// Clear the queue
    pub fn clear(&mut self) {
        self.queue.clear();
        self.index.clear();
        debug!("Pending queue cleared");
    }
    
    /// Rebuild the index after queue modifications
    fn rebuild_index(&mut self) {
        self.index.clear();
        for (pos, pending) in self.queue.iter().enumerate() {
            self.index.insert(pending.transfer.id.clone(), pos);
        }
    }
    
    /// Get oldest transfer age in milliseconds
    pub fn oldest_age_ms(&self) -> Option<u64> {
        self.queue.front().map(|p| {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64;
            now.saturating_sub(p.enqueued_at)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_types::{TransferType, Vlc};
    
    fn create_test_transfer(id: &str) -> Transfer {
        let mut vlc = Vlc::new();
        vlc.entries.insert("node1".to_string(), 1);
        
        Transfer {
            id: id.to_string(),
            from: "alice".to_string(),
            to: "bob".to_string(),
            amount: 100,
            transfer_type: TransferType::FluxTransfer,
            resources: vec!["alice".to_string()],
            vlc,
            power: 10,
        }
    }
    
    #[test]
    fn test_enqueue_dequeue() {
        let mut queue = PendingQueue::new(10);
        let transfer = create_test_transfer("t1");
        
        assert!(queue.enqueue(transfer.clone()).is_ok());
        assert_eq!(queue.size(), 1);
        
        let dequeued = queue.dequeue("t1").unwrap();
        assert_eq!(dequeued.id, "t1");
        assert_eq!(queue.size(), 0);
    }
    
    #[test]
    fn test_queue_full() {
        let mut queue = PendingQueue::new(2);
        
        assert!(queue.enqueue(create_test_transfer("t1")).is_ok());
        assert!(queue.enqueue(create_test_transfer("t2")).is_ok());
        
        let result = queue.enqueue(create_test_transfer("t3"));
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), PendingQueueError::QueueFull(_)));
    }
    
    #[test]
    fn test_duplicate_transfer() {
        let mut queue = PendingQueue::new(10);
        let transfer = create_test_transfer("t1");
        
        assert!(queue.enqueue(transfer.clone()).is_ok());
        
        let result = queue.enqueue(transfer);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), PendingQueueError::DuplicateTransfer(_)));
    }
    
    #[test]
    fn test_dequeue_next() {
        let mut queue = PendingQueue::new(10);
        
        queue.enqueue(create_test_transfer("t1")).unwrap();
        queue.enqueue(create_test_transfer("t2")).unwrap();
        queue.enqueue(create_test_transfer("t3")).unwrap();
        
        let t1 = queue.dequeue_next().unwrap();
        assert_eq!(t1.id, "t1");
        
        let t2 = queue.dequeue_next().unwrap();
        assert_eq!(t2.id, "t2");
        
        assert_eq!(queue.size(), 1);
    }
    
    #[test]
    fn test_peek_next() {
        let mut queue = PendingQueue::new(10);
        queue.enqueue(create_test_transfer("t1")).unwrap();
        
        let peeked = queue.peek_next().unwrap();
        assert_eq!(peeked.id, "t1");
        assert_eq!(queue.size(), 1); // Size unchanged
    }
}

