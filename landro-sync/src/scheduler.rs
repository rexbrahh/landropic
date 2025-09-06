//! Transfer scheduling and prioritization

use priority_queue::PriorityQueue;
use std::cmp::Reverse;

/// Transfer priority levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TransferPriority {
    Low = 0,
    Normal = 1,
    High = 2,
    Critical = 3,
}

/// Transfer request
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TransferRequest {
    pub peer_id: String,
    pub chunk_hash: String,
    pub file_path: String,
    pub size: u64,
}

/// Transfer scheduler
pub struct TransferScheduler {
    queue: PriorityQueue<TransferRequest, TransferPriority>,
    max_concurrent: usize,
    active_transfers: usize,
}

impl TransferScheduler {
    pub fn new(max_concurrent: usize) -> Self {
        Self {
            queue: PriorityQueue::new(),
            max_concurrent,
            active_transfers: 0,
        }
    }

    pub fn schedule(&mut self, request: TransferRequest, priority: TransferPriority) {
        self.queue.push(request, priority);
    }

    pub fn next_transfer(&mut self) -> Option<TransferRequest> {
        if self.active_transfers >= self.max_concurrent {
            return None;
        }

        self.queue.pop().map(|(request, _priority)| {
            self.active_transfers += 1;
            request
        })
    }

    pub fn complete_transfer(&mut self) {
        if self.active_transfers > 0 {
            self.active_transfers -= 1;
        }
    }

    pub fn pending_count(&self) -> usize {
        self.queue.len()
    }

    pub fn active_count(&self) -> usize {
        self.active_transfers
    }
}
