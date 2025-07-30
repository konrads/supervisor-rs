//! A simple supervisor function wrapper for managing groups of asynchronous tasks in Rust.
//! This module provides a way to spawn tasks that can be monitored and controlled,
//! allowing for graceful shutdown and management of task lifetimes.

use std::future::Future;
use tokio_util::sync::CancellationToken;

/// Wraps a future with a cancellation token, allowing it to be cancelled
pub async fn supervised<Fut>(fut: Fut, token: CancellationToken)
where
    Fut: Future<Output = ()> + Send + 'static,
{
    let token1 = token.clone();

    // Spawn the future into a task so we can catch panics via JoinHandle
    let handle = tokio::spawn(async move {
        tokio::select! {
            biased;

            _ = token1.cancelled() => {
                // External cancellation
            }
            _ = fut => {
                eprintln!("Task completed successfully, cancelling token");
                token1.cancel();
            }
        }
    });

    if handle.await.is_err() {
        eprintln!("Cancelling token due to a panic");
        token.cancel()
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
        let h1 = tokio::spawn(supervised(
            async move {
                sleep(10).await;
                state1.write().await.insert("task1");
            },
            cancel_token.clone(),
        ));

        let state2 = state.clone();
        let h2 = tokio::spawn(supervised(
            async move {
                sleep(20).await;
                state2.write().await.insert("task2-should-not-see");
            },
            cancel_token.clone(),
        ));

        sleep(15).await;

        let state_read = state.read().await;
        assert_eq!(state_read.clone(), HashSet::from(["task1"]));
        assert_eq!([true, true], [h1.is_finished(), h2.is_finished()]);
    }

    #[tokio::test]
    async fn test_explicit_supervisor_shutdown() {
        let cancel_token = CancellationToken::new();
        let state = Arc::new(RwLock::new(HashSet::new()));

        let state1 = state.clone();
        let h1 = tokio::spawn(supervised(
            async move {
                state1.write().await.insert("task1");
                sleep(10).await;
                state1.write().await.insert("task1-should-not-see");
            },
            cancel_token.clone(),
        ));

        let state2 = state.clone();
        let h2 = tokio::spawn(supervised(
            async move {
                state2.write().await.insert("task2");
                sleep(10).await;
                state2.write().await.insert("task2-should-not-see");
            },
            cancel_token.clone(),
        ));

        sleep(5).await;
        cancel_token.cancel();
        sleep(10).await;

        let state_read = state.read().await;
        assert_eq!(state_read.clone(), HashSet::from(["task1", "task2"]));
        assert_eq!([true, true], [h1.is_finished(), h2.is_finished()]);
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
                    sleep(5).await;
                    state1.write().await.insert("task1-should-not-see");
                },
                cancel_token.clone(),
            ));

            let state2 = state.clone();
            let _h2 = tokio::spawn(supervised(
                async move {
                    state2.write().await.insert("task2");
                    sleep(10).await;
                    state2.write().await.insert("task2-should-not-see");
                },
                cancel_token.clone(),
            ));

            sleep(2).await;
            // _h1 drops here
        }

        sleep(20).await;

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
                sleep(5).await;
                panic!("task1 panicked");
            },
            cancel_token.clone(),
        ));

        let state2 = state.clone();
        let _h2 = tokio::spawn(supervised(
            async move {
                state2.write().await.insert("task2");
                sleep(10).await;
                state2.write().await.insert("task2-should-not-see");
            },
            cancel_token.clone(),
        ));

        sleep(50).await;
        let state_read = state.read().await;
        assert_eq!(state_read.clone(), HashSet::from(["task1", "task2"]));
    }

    async fn sleep(ms: u64) {
        tokio::time::sleep(Duration::from_millis(ms)).await;
    }
}
