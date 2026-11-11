use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;

type TaskFn = Arc<dyn Fn() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

#[derive(Clone)]
pub(crate) struct ScheduledJob {
    pub name: String,
    pub interval: Duration,
    pub job: TaskFn,
}

/// Manages recurring background tasks and fire-and-forget async jobs.
#[derive(Clone, Default)]
pub struct TaskScheduler {
    pub(crate) jobs: Vec<ScheduledJob>,
}

impl TaskScheduler {
    pub fn new() -> Self {
        Self { jobs: Vec::new() }
    }

    /// Registers a recurring async task that runs every `interval`.
    pub fn every<F, Fut>(mut self, name: impl Into<String>, interval: Duration, task: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        self.jobs.push(ScheduledJob {
            name: name.into(),
            interval,
            job: Arc::new(move || Box::pin(task())),
        });
        self
    }

    /// Spawns an immediate background task onto the Tokio runtime.
    pub fn spawn<Fut>(&self, task: Fut)
    where
        Fut: Future<Output = ()> + Send + 'static,
    {
        tokio::task::spawn(task);
    }

    /// Starts all registered interval workers until `shutdown_rx` signals termination.
    pub(crate) fn start_workers(&self, shutdown_rx: watch::Receiver<bool>) {
        for scheduled in &self.jobs {
            let name = scheduled.name.clone();
            let interval_dur = scheduled.interval;
            let job = scheduled.job.clone();
            let mut rx = shutdown_rx.clone();

            tokio::task::spawn(async move {
                let mut ticker = tokio::time::interval(interval_dur);
                ticker.tick().await; // Skip immediate first tick at t=0

                loop {
                    tokio::select! {
                        _ = ticker.tick() => {
                            (job)().await;
                        }
                        _ = rx.changed() => {
                            println!("[TASKS] Stopping scheduled job '{}'", name);
                            break;
                        }
                    }
                }
            });
        }
    }
}
