//! A simple supervisor function wrapper for managing groups of asynchronous tasks in Rust.
//! This module provides a way to spawn tasks that can be monitored and controlled,
//! allowing for graceful shutdown and management of task lifetimes.

use std::future::Future;
use tokio_util::sync::CancellationToken;

/// Wraps a future with a cancellation token, allowing it to be cancelled
pub async fn supervised<F>(fut: F, token: CancellationToken)
where
    F: Future<Output = ()>,
{
    let local_token = token.clone();

    tokio::select! {
        _ = fut => {
            local_token.cancel(); // task completed, cancel others
        }
        _ = token.cancelled() => {
            // externally cancelled — exit early
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::Arc;
    use tokio::sync::RwLock;
    use tokio::time::Duration;

    #[tokio::test]
    async fn test_task_exit_kills_group() {
        let cancel_token = CancellationToken::new();
        let state = Arc::new(RwLock::new(HashSet::new()));

        let state1 = state.clone();
        let _h1 = tokio::spawn(supervised(
            async move {
                sleep(5, false).await;
                state1.write().await.insert("task1");
            },
            cancel_token.clone(),
        ));

        let state2 = state.clone();
        let _h2 = tokio::spawn(supervised(
            async move {
                sleep(10, false).await;
                state2.write().await.insert("task2-should-not-see");
            },
            cancel_token.clone(),
        ));

        let state3 = state.clone();
        let _h3 = tokio::spawn(supervised(
            async move {
                sleep(20, false).await;
                state3.write().await.insert("task3-should-not-see");
            },
            cancel_token.clone(),
        ));

        sleep(100, true).await;

        let state_read = state.read().await;
        assert_eq!(state_read.clone(), HashSet::from(["task1"]));
    }

    #[tokio::test]
    async fn test_explicit_supervisor_shutdown() {
        let cancel_token = CancellationToken::new();
        let state = Arc::new(RwLock::new(HashSet::new()));

        let state1 = state.clone();
        let _h1 = tokio::spawn(supervised(
            async move {
                state1.write().await.insert("task1");
                sleep(10, false).await;
                state1.write().await.insert("task1-should-not-see");
            },
            cancel_token.clone(),
        ));

        let state2 = state.clone();
        let _h2 = tokio::spawn(supervised(
            async move {
                state2.write().await.insert("task2");
                sleep(10, false).await;
                state2.write().await.insert("task2-should-not-see");
            },
            cancel_token.clone(),
        ));

        sleep(2, true).await;
        cancel_token.cancel();
        sleep(15, true).await;

        let state_read = state.read().await;
        assert_eq!(state_read.clone(), HashSet::from(["task1", "task2"]));
    }

    #[tokio::test]
    async fn test_drop_joinhandle_kills_group() {
        let cancel_token = CancellationToken::new();
        let state = Arc::new(RwLock::new(HashSet::new()));

        {
            let state1 = state.clone();
            let _h1 = tokio::spawn(supervised(
                async move {
                    state1.write().await.insert("task1");
                    sleep(5, false).await;
                    state1.write().await.insert("task1-should-not-see");
                },
                cancel_token.clone(),
            ));

            let state2 = state.clone();
            let _h2 = tokio::spawn(supervised(
                async move {
                    state2.write().await.insert("task2");
                    sleep(10, false).await;
                    state2.write().await.insert("task2-should-not-see");
                },
                cancel_token.clone(),
            ));

            sleep(2, true).await;
            // _h1 drops here
        }

        sleep(20, true).await;

        let state_read = state.read().await;
        assert_eq!(state_read.clone(), HashSet::from(["task1", "task2"]));
    }

    // create a test for task that panics
    #[tokio::test]
    async fn test_task_panic() {
        let cancel_token = CancellationToken::new();
        let state = Arc::new(RwLock::new(HashSet::new()));

        let state1 = state.clone();
        let _h1 = tokio::spawn(supervised(
            async move {
                state1.write().await.insert("task1");
                sleep(5, false).await;
                panic!("task1 panicked");
            },
            cancel_token.clone(),
        ));

        let state2 = state.clone();
        let _h2 = tokio::spawn(supervised(
            async move {
                state2.write().await.insert("task2");
                sleep(10, false).await;
                state2.write().await.insert("task2-should-not-see");
            },
            cancel_token.clone(),
        ));

        sleep(20, true).await;
        let state_read = state.read().await;
        assert_eq!(state_read.clone(), HashSet::from(["task1", "task2"]));
    }

    async fn sleep(ms: u64, all_in_1_go: bool) {
        if all_in_1_go {
            tokio::time::sleep(Duration::from_millis(ms)).await;
        } else {
            for _ in 0..ms {
                tokio::time::sleep(Duration::from_millis(ms)).await;
            }
        }
    }
}
