use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tracing::{info, warn};

pub const WATCHDOG_INTERVAL: Duration = Duration::from_secs(60);
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(3 * 60);

#[derive(Clone, Default)]
pub struct ActivityTracker {
    pub last_seen: Arc<Mutex<Option<Instant>>>,
    pub active_requests: Arc<AtomicUsize>,
}

impl ActivityTracker {
    pub fn touch(&self) {
        *self.last_seen.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
    }

    pub fn increment(&self) {
        self.active_requests.fetch_add(1, Ordering::SeqCst);
    }

    pub fn decrement(&self) {
        self.active_requests.fetch_sub(1, Ordering::SeqCst);
    }
}

pub fn should_shutdown(last_seen: Option<Instant>, active: usize, timeout: Duration) -> bool {
    match last_seen {
        None => false,
        Some(t) => active == 0 && t.elapsed() > timeout,
    }
}

/// Spawns a background task that stops the RunPod pod when idle.
/// Reads RUNPOD_POD_ID and RUNPOD_USER_API_KEY from the environment.
/// No-op if either var is unset (local dev).
pub fn spawn_idle_watchdog(tracker: ActivityTracker) {
    let pod_id = std::env::var("RUNPOD_POD_ID").ok();
    let api_key = std::env::var("RUNPOD_USER_API_KEY").ok();

    if pod_id.is_none() || api_key.is_none() {
        warn!("watchdog: RUNPOD_POD_ID or RUNPOD_USER_API_KEY not set — idle auto-stop disabled");
        return;
    }

    let pod_id = pod_id.unwrap();
    let api_key = api_key.unwrap();

    tokio::spawn(async move {
        loop {
            tokio::time::sleep(WATCHDOG_INTERVAL).await;

            let last_seen = *tracker.last_seen.lock().unwrap_or_else(|e| e.into_inner());
            let active = tracker.active_requests.load(Ordering::SeqCst);

            if should_shutdown(last_seen, active, IDLE_TIMEOUT) {
                info!("watchdog: idle for {:?}, stopping pod {pod_id}", IDLE_TIMEOUT);
                stop_pod(&pod_id, &api_key).await;
                // RunPod will stop the container; don't exit ourselves so
                // the restart policy doesn't respawn before stop takes effect.
                break;
            }
        }
    });
}

async fn stop_pod(pod_id: &str, api_key: &str) {
    let client = reqwest::Client::new();
    match client
        .post(format!("https://rest.runpod.io/v1/pods/{pod_id}/stop"))
        .bearer_auth(api_key)
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => info!("watchdog: stop request sent for pod {pod_id}"),
        Ok(r) => warn!("watchdog: stop request failed: HTTP {}", r.status()),
        Err(e) => warn!("watchdog: stop request error: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_connected_never_shuts_down() {
        assert!(!should_shutdown(None, 0, IDLE_TIMEOUT));
    }

    #[test]
    fn active_request_does_not_shut_down() {
        assert!(!should_shutdown(Some(Instant::now() - Duration::from_secs(9999)), 1, IDLE_TIMEOUT));
    }

    #[test]
    fn idle_past_timeout_shuts_down() {
        let old = Some(Instant::now() - Duration::from_secs(16 * 60));
        assert!(should_shutdown(old, 0, IDLE_TIMEOUT));
    }

    #[test]
    fn recent_activity_does_not_shut_down() {
        assert!(!should_shutdown(Some(Instant::now()), 0, IDLE_TIMEOUT));
    }
}
