//! A simple supervisor for managing groups of asynchronous tasks in Rust.
//! This module provides a way to spawn tasks that can be monitored and controlled,
//! allowing for graceful shutdown and management of task lifetimes.

use futures::future::{AbortHandle, Abortable};
use parking_lot::Mutex;
use std::{future::Future, sync::Arc};
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

/// SupervisorHandle
#[derive(Clone)]
pub struct SupervisorHandle {
    aborts: Arc<Mutex<Vec<AbortHandle>>>,
    shutdown: broadcast::Sender<()>,
}

impl Default for SupervisorHandle {
    fn default() -> Self {
        Self::new()
    }
}

impl SupervisorHandle {
    pub fn new() -> Self {
        let (shutdown, _) = broadcast::channel(1);
        Self {
            aborts: Arc::new(Mutex::new(Vec::new())),
            shutdown,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<()> {
        self.shutdown.subscribe()
    }

    pub fn register(&self, handle: AbortHandle) {
        self.aborts.lock().push(handle);
    }

    pub fn shutdown_all(&self) {
        let _ = self.shutdown.send(());
        for handle in self.aborts.lock().drain(..) {
            handle.abort();
        }
    }
}

/// Supervisor logic wrapping futures
pub fn supervise<F>(
    fut: F,
    supervisor: SupervisorHandle,
) -> impl Future<Output = ()> + Send + 'static
where
    F: Future<Output = ()> + Send + 'static,
{
    let (abort_handle, abort_reg) = AbortHandle::new_pair();
    supervisor.register(abort_handle);
    let mut shutdown_rx = supervisor.subscribe();

    let abortable = Abortable::new(
        async move {
            tokio::select! {
                biased;
                _ = shutdown_rx.recv() => {}
                _ = fut => {
                    supervisor.shutdown_all();
                }
            }
        },
        abort_reg,
    );

    async move {
        let _ = abortable.await;
    }
}

/// JoinHandle wrapper that triggers shutdown on drop
pub struct SupervisedJoinHandle {
    inner: JoinHandle<()>,
    supervisor: Option<SupervisorHandle>,
}

impl SupervisedJoinHandle {
    pub fn new(inner: JoinHandle<()>, supervisor: SupervisorHandle) -> Self {
        Self {
            inner,
            supervisor: Some(supervisor),
        }
    }
}

impl Drop for SupervisedJoinHandle {
    fn drop(&mut self) {
        if let Some(sup) = self.supervisor.take() {
            sup.shutdown_all();
        }
    }
}

impl Future for SupervisedJoinHandle {
    type Output = ();

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        self.supervisor = None;
        std::pin::Pin::new(&mut self.inner).poll(cx).map(|_| ())
    }
}

// === External interface ===
pub fn supervise_spawn<F>(fut: F, supervisor: SupervisorHandle) -> SupervisedJoinHandle
where
    F: Future<Output = ()> + Send + 'static,
{
    let inner = tokio::spawn(supervise(fut, supervisor.clone()));
    SupervisedJoinHandle::new(inner, supervisor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::Arc;
    use tokio::sync::RwLock;
    use tokio::time::{Duration, sleep};

    #[tokio::test]
    async fn test_task_exit_kills_group() {
        let supervisor = SupervisorHandle::new();
        let state = Arc::new(RwLock::new(HashSet::new()));

        let state1 = state.clone();
        let _h1 = supervise_spawn(
            async move {
                sleep(Duration::from_millis(5)).await;
                state1.write().await.insert("task1");
            },
            supervisor.clone(),
        );

        let state2 = state.clone();
        let _h2 = supervise_spawn(
            async move {
                sleep(Duration::from_millis(10)).await;
                state2.write().await.insert("task2-should-not-see");
            },
            supervisor.clone(),
        );

        let state3 = state.clone();
        let _h3 = supervise_spawn(
            async move {
                sleep(Duration::from_millis(20)).await;
                state3.write().await.insert("task3-should-not-see");
            },
            supervisor.clone(),
        );

        sleep(Duration::from_millis(100)).await;

        let state_read = state.read().await;
        assert_eq!(state_read.clone(), HashSet::from(["task1"]));
    }

    #[tokio::test]
    async fn test_explicit_supervisor_shutdown() {
        let supervisor = SupervisorHandle::new();
        let state = Arc::new(RwLock::new(HashSet::new()));

        let state1 = state.clone();
        let _h1 = supervise_spawn(
            async move {
                state1.write().await.insert("task1");
                sleep(Duration::from_millis(10)).await;
                state1.write().await.insert("task1-should-not-see");
            },
            supervisor.clone(),
        );

        let state2 = state.clone();
        let _h2 = supervise_spawn(
            async move {
                state2.write().await.insert("task2");
                sleep(Duration::from_millis(10)).await;
                state2.write().await.insert("task2-should-not-see");
            },
            supervisor.clone(),
        );

        sleep(Duration::from_millis(2)).await;
        supervisor.shutdown_all();
        sleep(Duration::from_millis(15)).await;

        let state_read = state.read().await;
        assert_eq!(state_read.clone(), HashSet::from(["task1", "task2"]));
    }

    #[tokio::test]
    async fn test_drop_joinhandle_kills_group() {
        let supervisor = SupervisorHandle::new();
        let state = Arc::new(RwLock::new(HashSet::new()));

        {
            let state1 = state.clone();
            let _h1 = supervise_spawn(
                async move {
                    state1.write().await.insert("task1");
                    sleep(Duration::from_millis(5)).await;
                    state1.write().await.insert("task1-should-not-see");
                },
                supervisor.clone(),
            );

            let state2 = state.clone();
            let _h2 = supervise_spawn(
                async move {
                    state2.write().await.insert("task2");
                    sleep(Duration::from_millis(10)).await;
                    state2.write().await.insert("task2-should-not-see");
                },
                supervisor.clone(),
            );

            sleep(Duration::from_millis(2)).await;
            // _h1 drops here
        }

        sleep(Duration::from_millis(20)).await;

        let state_read = state.read().await;
        assert_eq!(state_read.clone(), HashSet::from(["task1", "task2"]));
    }
}
