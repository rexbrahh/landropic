use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;
use notify::{Watcher, RecommendedWatcher, RecursiveMode, Event, EventKind};
use tokio::sync::Mutex;
use tracing::{info, warn, error, debug};

/// File system change event
#[derive(Debug, Clone)]
pub struct FileEvent {
    pub path: PathBuf,
    pub kind: FileEventKind,
}

/// Type of file system change
#[derive(Debug, Clone, PartialEq)]
pub enum FileEventKind {
    Created,
    Modified,
    Deleted,
    Renamed { from: PathBuf, to: PathBuf },
}

/// File system watcher with callback support
pub struct FileWatcher {
    path: PathBuf,
    _watcher: Option<RecommendedWatcher>,
    running: Arc<Mutex<bool>>,
}

impl FileWatcher {
    /// Create new file watcher for a directory
    pub fn new(path: PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
        Ok(Self {
            path,
            _watcher: None,
            running: Arc::new(Mutex::new(false)),
        })
    }

    /// Get the path being watched
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Start watching with a callback
    pub fn start<F>(&self, callback: F) -> Result<(), Box<dyn std::error::Error>>
    where
        F: Fn(Vec<FileEvent>) + Send + 'static,
    {
        let (tx, rx) = mpsc::channel();
        let path = self.path.clone();
        let running = self.running.clone();

        // Create notify watcher
        let mut watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
            match res {
                Ok(event) => {
                    if let Err(e) = tx.send(event) {
                        error!("Failed to send file event: {}", e);
                    }
                }
                Err(e) => {
                    error!("File watcher error: {}", e);
                }
            }
        })?;

        // Start watching
        watcher.watch(&path, RecursiveMode::Recursive)?;
        
        info!("Started watching directory: {}", path.display());

        // Spawn background task to handle events
        tokio::spawn(async move {
            let mut running_guard = running.lock().await;
            *running_guard = true;
            drop(running_guard);

            // Event batching to reduce noise
            let mut pending_events = Vec::new();
            let mut last_batch_time = std::time::Instant::now();
            const BATCH_TIMEOUT: Duration = Duration::from_millis(100);

            loop {
                // Check if we should stop
                if !*running.lock().await {
                    break;
                }

                // Try to receive events with timeout
                match rx.recv_timeout(BATCH_TIMEOUT) {
                    Ok(event) => {
                        if let Some(file_events) = convert_notify_event(event) {
                            pending_events.extend(file_events);
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        // Process batched events if we have any and timeout elapsed
                        if !pending_events.is_empty() && last_batch_time.elapsed() >= BATCH_TIMEOUT {
                            let events = std::mem::take(&mut pending_events);
                            debug!("Processing {} batched file events", events.len());
                            callback(events);
                            last_batch_time = std::time::Instant::now();
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        warn!("File watcher disconnected");
                        break;
                    }
                }
            }

            // Process any remaining events
            if !pending_events.is_empty() {
                callback(pending_events);
            }

            info!("File watcher stopped for: {}", path.display());
        });

        Ok(())
    }

    /// Stop watching
    pub fn stop(&self) -> Result<(), Box<dyn std::error::Error>> {
        tokio::spawn({
            let running = self.running.clone();
            async move {
                let mut guard = running.lock().await;
                *guard = false;
            }
        });

        info!("Stopping file watcher for: {}", self.path.display());
        Ok(())
    }

    /// Check if watcher is running
    pub async fn is_running(&self) -> bool {
        *self.running.lock().await
    }
}

/// Convert notify event to our file event format
fn convert_notify_event(event: Event) -> Option<Vec<FileEvent>> {
    let mut file_events = Vec::new();

    // Filter out temporary files and hidden files
    let should_ignore = |path: &Path| -> bool {
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            // Ignore hidden files
            if name.starts_with('.') {
                return true;
            }
            
            // Ignore temporary files
            if name.ends_with('~') || name.ends_with(".tmp") || name.ends_with(".swp") {
                return true;
            }
            
            // Ignore common editor temp files
            if name.starts_with("#") && name.ends_with("#") {
                return true;
            }
        }
        
        false
    };

    for path in &event.paths {
        if should_ignore(path) {
            continue;
        }

        let file_event = match event.kind {
            EventKind::Create(_) => FileEvent {
                path: path.clone(),
                kind: FileEventKind::Created,
            },
            EventKind::Modify(_) => FileEvent {
                path: path.clone(),
                kind: FileEventKind::Modified,
            },
            EventKind::Remove(_) => FileEvent {
                path: path.clone(),
                kind: FileEventKind::Deleted,
            },
            _ => continue, // Ignore other event types for now
        };

        file_events.push(file_event);
    }

    if file_events.is_empty() {
        None
    } else {
        Some(file_events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;
    use tokio::time::{sleep, Duration};

    #[tokio::test]
    async fn test_file_watcher() {
        let temp_dir = TempDir::new().unwrap();
        let temp_path = temp_dir.path().to_path_buf();
        
        let watcher = FileWatcher::new(temp_path.clone()).unwrap();
        
        let events = Arc::new(Mutex::new(Vec::new()));
        let events_clone = events.clone();
        
        // Start watching
        watcher.start(move |file_events| {
            let events = events_clone.clone();
            tokio::spawn(async move {
                let mut guard = events.lock().await;
                guard.extend(file_events);
            });
        }).unwrap();

        // Give watcher time to start
        sleep(Duration::from_millis(100)).await;

        // Create a test file
        let test_file = temp_path.join("test.txt");
        fs::write(&test_file, b"hello world").unwrap();

        // Give time for events to be processed
        sleep(Duration::from_millis(200)).await;

        // Check we received create event
        let events_guard = events.lock().await;
        assert!(!events_guard.is_empty());
        assert!(events_guard.iter().any(|e| e.kind == FileEventKind::Created));

        // Stop watcher
        watcher.stop().unwrap();
        sleep(Duration::from_millis(100)).await;
        assert!(!watcher.is_running().await);
    }
}