//! Connection recovery and error handling for QUIC operations

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tokio::time::sleep;
use tracing::{debug, error, info, warn};

use crate::client::QuicClient;
use crate::connection::Connection;
use crate::errors::{QuicError, Result};

/// Retry policy for connection operations
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// Maximum number of retry attempts
    pub max_attempts: u32,
    /// Base delay between retries
    pub base_delay: Duration,
    /// Maximum delay between retries
    pub max_delay: Duration,
    /// Backoff multiplier (exponential backoff)
    pub backoff_multiplier: f64,
    /// Jitter factor to randomize delays (0.0 - 1.0)
    pub jitter_factor: f64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(30),
            backoff_multiplier: 2.0,
            jitter_factor: 0.1,
        }
    }
}

impl RetryPolicy {
    /// Create a more aggressive retry policy
    pub fn aggressive() -> Self {
        Self {
            max_attempts: 5,
            base_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(10),
            backoff_multiplier: 1.5,
            jitter_factor: 0.2,
        }
    }

    /// Create a conservative retry policy
    pub fn conservative() -> Self {
        Self {
            max_attempts: 2,
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(60),
            backoff_multiplier: 3.0,
            jitter_factor: 0.05,
        }
    }

    /// Calculate delay for a given attempt number
    pub fn calculate_delay(&self, attempt: u32) -> Duration {
        if attempt == 0 {
            return Duration::ZERO;
        }

        let base_delay_ms = self.base_delay.as_millis() as f64;
        let multiplier = self.backoff_multiplier.powi(attempt as i32 - 1);
        let delay_ms = base_delay_ms * multiplier;

        // Apply maximum delay cap
        let delay_ms = delay_ms.min(self.max_delay.as_millis() as f64);

        // Apply jitter to avoid thundering herd
        let jitter = 1.0 + (rand::random::<f64>() - 0.5) * 2.0 * self.jitter_factor;
        let final_delay_ms = delay_ms * jitter;

        Duration::from_millis(final_delay_ms.max(0.0) as u64)
    }
}

/// Circuit breaker state for connection health management
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CircuitState {
    Closed,   // Normal operation
    Open,     // Failing, reject requests
    HalfOpen, // Testing if service recovered
}

/// Circuit breaker for connection failure detection
#[derive(Debug)]
pub struct CircuitBreaker {
    state: CircuitState,
    failure_count: u32,
    success_count: u32,
    last_failure: Option<Instant>,

    // Configuration
    failure_threshold: u32,
    success_threshold: u32,
    timeout: Duration,
}

impl CircuitBreaker {
    pub fn new(failure_threshold: u32, success_threshold: u32, timeout: Duration) -> Self {
        Self {
            state: CircuitState::Closed,
            failure_count: 0,
            success_count: 0,
            last_failure: None,
            failure_threshold,
            success_threshold,
            timeout,
        }
    }

    /// Check if requests should be allowed through
    pub fn allow_request(&mut self) -> bool {
        match self.state {
            CircuitState::Closed => true,
            CircuitState::Open => {
                if let Some(last_failure) = self.last_failure {
                    if last_failure.elapsed() > self.timeout {
                        debug!("Circuit breaker transitioning to half-open");
                        self.state = CircuitState::HalfOpen;
                        self.success_count = 0;
                        true
                    } else {
                        false
                    }
                } else {
                    true
                }
            }
            CircuitState::HalfOpen => true,
        }
    }

    /// Record a successful operation
    pub fn record_success(&mut self) {
        match self.state {
            CircuitState::Closed => {
                self.failure_count = 0;
            }
            CircuitState::HalfOpen => {
                self.success_count += 1;
                if self.success_count >= self.success_threshold {
                    info!("Circuit breaker transitioning to closed (recovered)");
                    self.state = CircuitState::Closed;
                    self.failure_count = 0;
                    self.success_count = 0;
                    self.last_failure = None;
                }
            }
            CircuitState::Open => {
                // Should not happen, but reset if it does
                self.state = CircuitState::Closed;
                self.failure_count = 0;
                self.success_count = 0;
                self.last_failure = None;
            }
        }
    }

    /// Record a failed operation
    pub fn record_failure(&mut self) {
        self.last_failure = Some(Instant::now());

        match self.state {
            CircuitState::Closed => {
                self.failure_count += 1;
                if self.failure_count >= self.failure_threshold {
                    warn!(
                        "Circuit breaker opening due to {} failures",
                        self.failure_count
                    );
                    self.state = CircuitState::Open;
                }
            }
            CircuitState::HalfOpen => {
                warn!("Circuit breaker reopening after half-open failure");
                self.state = CircuitState::Open;
                self.success_count = 0;
            }
            CircuitState::Open => {
                // Already open, just update timestamp
            }
        }
    }

    pub fn state(&self) -> CircuitState {
        self.state
    }
}

/// Recovery-aware QUIC client that handles failures gracefully
pub struct RecoveryClient {
    inner: Arc<QuicClient>,
    circuit_breakers: Arc<RwLock<HashMap<std::net::SocketAddr, CircuitBreaker>>>,
    retry_policy: RetryPolicy,
}

impl RecoveryClient {
    /// Create a new recovery client
    pub fn new(client: Arc<QuicClient>, retry_policy: RetryPolicy) -> Self {
        Self {
            inner: client,
            circuit_breakers: Arc::new(RwLock::new(HashMap::new())),
            retry_policy,
        }
    }

    /// Connect with automatic retry and circuit breaking
    pub async fn connect_with_recovery(
        &self,
        addr: std::net::SocketAddr,
    ) -> Result<Arc<Connection>> {
        // Check circuit breaker
        {
            let mut breakers = self.circuit_breakers.write().await;
            let breaker = breakers
                .entry(addr)
                .or_insert_with(|| CircuitBreaker::new(3, 2, Duration::from_secs(30)));

            if !breaker.allow_request() {
                return Err(QuicError::Protocol(format!(
                    "Circuit breaker open for peer {}, rejecting request",
                    addr
                )));
            }
        }

        let mut last_error = None;

        for attempt in 0..self.retry_policy.max_attempts {
            if attempt > 0 {
                let delay = self.retry_policy.calculate_delay(attempt);
                debug!(
                    "Retrying connection to {} after {:?} (attempt {}/{})",
                    addr,
                    delay,
                    attempt + 1,
                    self.retry_policy.max_attempts
                );
                sleep(delay).await;
            }

            match self.inner.connect_addr(addr).await {
                Ok(connection) => {
                    info!("Connected to {} on attempt {}", addr, attempt + 1);

                    // Record success in circuit breaker
                    {
                        let mut breakers = self.circuit_breakers.write().await;
                        if let Some(breaker) = breakers.get_mut(&addr) {
                            breaker.record_success();
                        }
                    }

                    return Ok(Arc::new(connection));
                }
                Err(e) => {
                    error!(
                        "Connection attempt {} to {} failed: {}",
                        attempt + 1,
                        addr,
                        e
                    );
                    last_error = Some(e.clone());

                    // Record failure in circuit breaker
                    {
                        let mut breakers = self.circuit_breakers.write().await;
                        if let Some(breaker) = breakers.get_mut(&addr) {
                            breaker.record_failure();
                        }
                    }

                    // Don't retry for non-recoverable errors
                    if !e.is_recoverable() {
                        warn!("Non-recoverable error, not retrying: {}", e);
                        break;
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            QuicError::Protocol(format!(
                "All {} connection attempts failed",
                self.retry_policy.max_attempts
            ))
        }))
    }

    /// Execute an operation with retry logic
    pub async fn with_retry<F, T, Fut>(&self, addr: std::net::SocketAddr, operation: F) -> Result<T>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<T>>,
    {
        let mut last_error = None;

        for attempt in 0..self.retry_policy.max_attempts {
            if attempt > 0 {
                let delay = self.retry_policy.calculate_delay(attempt);
                debug!(
                    "Retrying operation for {} after {:?} (attempt {}/{})",
                    addr,
                    delay,
                    attempt + 1,
                    self.retry_policy.max_attempts
                );
                sleep(delay).await;
            }

            match operation().await {
                Ok(result) => {
                    if attempt > 0 {
                        info!(
                            "Operation succeeded for {} on attempt {}",
                            addr,
                            attempt + 1
                        );
                    }

                    // Record success in circuit breaker
                    {
                        let mut breakers = self.circuit_breakers.write().await;
                        if let Some(breaker) = breakers.get_mut(&addr) {
                            breaker.record_success();
                        }
                    }

                    return Ok(result);
                }
                Err(e) => {
                    debug!(
                        "Operation attempt {} for {} failed: {}",
                        attempt + 1,
                        addr,
                        e
                    );
                    last_error = Some(e.clone());

                    // Record failure in circuit breaker
                    {
                        let mut breakers = self.circuit_breakers.write().await;
                        if let Some(breaker) = breakers.get_mut(&addr) {
                            breaker.record_failure();
                        }
                    }

                    // Don't retry for non-recoverable errors
                    if !e.is_recoverable() {
                        debug!("Non-recoverable error, not retrying: {}", e);
                        break;
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            QuicError::Protocol(format!(
                "All {} operation attempts failed",
                self.retry_policy.max_attempts
            ))
        }))
    }

    /// Get circuit breaker states for monitoring
    pub async fn get_circuit_states(&self) -> HashMap<std::net::SocketAddr, CircuitState> {
        let breakers = self.circuit_breakers.read().await;
        breakers
            .iter()
            .map(|(addr, breaker)| (*addr, breaker.state()))
            .collect()
    }

    /// Reset circuit breaker for a specific address
    pub async fn reset_circuit_breaker(&self, addr: std::net::SocketAddr) {
        let mut breakers = self.circuit_breakers.write().await;
        breakers.remove(&addr);
        info!("Reset circuit breaker for {}", addr);
    }
}

/// Health monitor for connections
pub struct ConnectionHealthMonitor {
    connections: Arc<RwLock<HashMap<String, ConnectionHealth>>>,
    check_interval: Duration,
}

#[derive(Debug, Clone)]
struct ConnectionHealth {
    connection: Arc<Connection>,
    last_ping: Option<Instant>,
    ping_failures: u32,
    avg_rtt: Option<Duration>,
}

impl ConnectionHealthMonitor {
    pub fn new(check_interval: Duration) -> Self {
        Self {
            connections: Arc::new(RwLock::new(HashMap::new())),
            check_interval,
        }
    }

    /// Add a connection to monitor
    pub async fn monitor_connection(&self, id: String, connection: Arc<Connection>) {
        let health = ConnectionHealth {
            connection,
            last_ping: None,
            ping_failures: 0,
            avg_rtt: None,
        };

        let mut connections = self.connections.write().await;
        connections.insert(id, health);
    }

    /// Remove a connection from monitoring
    pub async fn unmonitor_connection(&self, id: &str) {
        let mut connections = self.connections.write().await;
        connections.remove(id);
    }

    /// Start background health checking
    pub async fn start_monitoring(&self) {
        let connections = self.connections.clone();
        let interval = self.check_interval;

        tokio::spawn(async move {
            let mut check_interval = tokio::time::interval(interval);
            loop {
                check_interval.tick().await;
                Self::check_all_connections(&connections).await;
            }
        });
    }

    async fn check_all_connections(connections: &Arc<RwLock<HashMap<String, ConnectionHealth>>>) {
        let connection_ids: Vec<String> = {
            let conns = connections.read().await;
            conns.keys().cloned().collect()
        };

        for id in connection_ids {
            if let Err(e) = Self::ping_connection(&connections, &id).await {
                warn!("Health check failed for connection {}: {}", id, e);
            }
        }
    }

    async fn ping_connection(
        connections: &Arc<RwLock<HashMap<String, ConnectionHealth>>>,
        id: &str,
    ) -> Result<()> {
        let connection = {
            let conns = connections.read().await;
            conns.get(id).map(|h| h.connection.clone())
        };

        if let Some(conn) = connection {
            if conn.is_closed() {
                let mut conns = connections.write().await;
                conns.remove(id);
                info!("Removed closed connection from health monitoring: {}", id);
                return Ok(());
            }

            // Perform basic connectivity check
            let start = Instant::now();

            // For now, just check if we can open a stream
            // In a real implementation, you might send a ping message
            match conn.open_uni().await {
                Ok(mut stream) => {
                    let _ = stream.finish(); // Close the stream immediately
                    let rtt = start.elapsed();

                    // Update health metrics
                    let mut conns = connections.write().await;
                    if let Some(health) = conns.get_mut(id) {
                        health.last_ping = Some(Instant::now());
                        health.ping_failures = 0;
                        health.avg_rtt = Some(match health.avg_rtt {
                            Some(existing) => Duration::from_nanos(
                                ((existing.as_nanos() + rtt.as_nanos()) / 2) as u64,
                            ),
                            None => rtt,
                        });
                    }

                    debug!("Connection {} health check passed (RTT: {:?})", id, rtt);
                }
                Err(e) => {
                    let mut conns = connections.write().await;
                    if let Some(health) = conns.get_mut(id) {
                        health.ping_failures += 1;

                        if health.ping_failures >= 3 {
                            warn!(
                                "Connection {} failed health check {} times, removing",
                                id, health.ping_failures
                            );
                            conns.remove(id);
                        }
                    }

                    return Err(e);
                }
            }
        }

        Ok(())
    }

    /// Get health statistics for all connections
    pub async fn get_health_stats(&self) -> HashMap<String, (u32, Option<Duration>)> {
        let connections = self.connections.read().await;
        connections
            .iter()
            .map(|(id, health)| (id.clone(), (health.ping_failures, health.avg_rtt)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_retry_policy_delay_calculation() {
        let policy = RetryPolicy::default();

        assert_eq!(policy.calculate_delay(0), Duration::ZERO);

        let delay1 = policy.calculate_delay(1);
        assert!(delay1 >= Duration::from_millis(450)); // With jitter
        assert!(delay1 <= Duration::from_millis(550));

        let delay2 = policy.calculate_delay(2);
        assert!(delay2 > delay1); // Should be longer due to backoff
    }

    #[test]
    fn test_circuit_breaker_state_transitions() {
        let mut breaker = CircuitBreaker::new(2, 1, Duration::from_secs(1));

        assert_eq!(breaker.state(), CircuitState::Closed);
        assert!(breaker.allow_request());

        // First failure
        breaker.record_failure();
        assert_eq!(breaker.state(), CircuitState::Closed);
        assert!(breaker.allow_request());

        // Second failure - should open
        breaker.record_failure();
        assert_eq!(breaker.state(), CircuitState::Open);
        assert!(!breaker.allow_request()); // Should be rejected

        // Success should close it when half-open
        breaker.state = CircuitState::HalfOpen;
        breaker.record_success();
        assert_eq!(breaker.state(), CircuitState::Closed);
    }
}
