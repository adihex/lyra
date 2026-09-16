//! Async-by-construction jobs: `operation.submit` for slow ops returns a
//! `{job:{id,operation,state}}` handle; clients poll `job.get` or subscribe to
//! `runtime.job`. Jobs retained ~15 min, store bounded (spec §1.2).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Retention window and capacity bound.
pub const RETENTION: Duration = Duration::from_secs(15 * 60);
pub const MAX_JOBS: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobState {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

impl JobState {
    pub fn terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobInfo {
    pub id: String,
    pub operation: String,
    pub state: JobState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub created: u64,
    pub updated: u64,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

struct Inner {
    jobs: HashMap<String, JobInfo>,
    order: VecDeque<String>,
    counter: u64,
}

pub struct JobStore {
    inner: Mutex<Inner>,
}

impl Default for JobStore {
    fn default() -> Self {
        Self::new()
    }
}

impl JobStore {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                jobs: HashMap::new(),
                order: VecDeque::new(),
                counter: 0,
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn evict_locked(inner: &mut Inner) {
        let cutoff = now_secs().saturating_sub(RETENTION.as_secs());
        while let Some(front) = inner.order.front() {
            let old = inner
                .jobs
                .get(front)
                .map(|j| !j.state.terminal() || j.updated < cutoff)
                .unwrap_or(true);
            let over = inner.jobs.len() > MAX_JOBS
                && inner
                    .jobs
                    .get(front)
                    .map(|j| j.state.terminal())
                    .unwrap_or(true);
            if !old && !over {
                break;
            }
            if let Some(id) = inner.order.pop_front() {
                inner.jobs.remove(&id);
            }
        }
    }

    /// Create a job in `queued` state. Returns the full [`JobInfo`].
    pub fn create(&self, operation: &str) -> JobInfo {
        let mut inner = self.lock();
        inner.counter += 1;
        let id = format!("job-{}", inner.counter);
        let now = now_secs();
        let job = JobInfo {
            id: id.clone(),
            operation: operation.to_string(),
            state: JobState::Queued,
            result: None,
            error: None,
            created: now,
            updated: now,
        };
        inner.order.push_back(id.clone());
        inner.jobs.insert(id.clone(), job.clone());
        Self::evict_locked(&mut inner);
        job
    }

    pub fn get(&self, id: &str) -> Option<JobInfo> {
        let mut inner = self.lock();
        Self::evict_locked(&mut inner);
        inner.jobs.get(id).cloned()
    }

    fn transition(&self, id: &str, from: &[JobState], to: JobState) -> Option<JobInfo> {
        let mut inner = self.lock();
        let job = inner.jobs.get_mut(id)?;
        if !from.contains(&job.state) {
            return None;
        }
        job.state = to;
        job.updated = now_secs();
        Some(job.clone())
    }

    pub fn set_running(&self, id: &str) -> Option<JobInfo> {
        self.transition(id, &[JobState::Queued], JobState::Running)
    }

    /// Mark succeeded. Only wins from `queued`/`running` — a concurrent
    /// `cancel` is never overwritten by a late worker finish.
    pub fn succeed(&self, id: &str, result: Value) -> Option<JobInfo> {
        let mut inner = self.lock();
        let job = inner.jobs.get_mut(id)?;
        if !matches!(job.state, JobState::Queued | JobState::Running) {
            return None;
        }
        job.state = JobState::Succeeded;
        job.result = Some(result);
        job.updated = now_secs();
        Some(job.clone())
    }

    pub fn fail(&self, id: &str, error: String) -> Option<JobInfo> {
        let mut inner = self.lock();
        let job = inner.jobs.get_mut(id)?;
        if !matches!(job.state, JobState::Queued | JobState::Running) {
            return None;
        }
        job.state = JobState::Failed;
        job.error = Some(error);
        job.updated = now_secs();
        Some(job.clone())
    }

    /// Cancel a non-terminal job. Returns the job, or `None` when unknown or
    /// already terminal.
    pub fn cancel(&self, id: &str) -> Option<JobInfo> {
        self.transition(
            id,
            &[JobState::Queued, JobState::Running],
            JobState::Cancelled,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_lifecycle_transitions() {
        let store = JobStore::new();
        let job = store.create("library.scan");
        assert_eq!(job.state, JobState::Queued);
        assert!(store.set_running(&job.id).is_some());
        // cancel wins over a late finish
        assert!(store.cancel(&job.id).is_some());
        assert!(store.succeed(&job.id, Value::Null).is_none());
        assert_eq!(store.get(&job.id).unwrap().state, JobState::Cancelled);
        // terminal jobs can't be cancelled again
        assert!(store.cancel(&job.id).is_none());
        assert!(store.get("nope").is_none());
    }
}
