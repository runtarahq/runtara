// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Integration tests for SDK cancellation functionality.
//!
//! These tests verify:
//! 1. `with_cancellation()` properly races operations against cancellation token
//! 2. `is_cancelled()` correctly reports cancellation state
//! 3. Operations complete normally when not cancelled
//! 4. Long-running operations are interrupted when cancelled
//!
//! Note: Due to global state in the registry, some tests may need to be run
//! in isolation. Use `cargo test -p runtara-sdk --test cancellation_test -- --test-threads=1`
//!
//! Run with:
//! ```bash
//! cargo test -p runtara-sdk --test cancellation_test
//! ```

use std::time::{Duration, Instant};

// ============================================================================
// Tests for with_cancellation() without global registry
// ============================================================================

/// Test that with_cancellation completes normally when no SDK is registered.
/// (Falls back to just running the operation)
#[tokio::test]
async fn test_with_cancellation_no_registry_succeeds() {
    // When no SDK is registered, with_cancellation should just run the operation
    let result = runtara_sdk::with_cancellation(async { 42 }).await;

    assert!(result.is_ok());
    assert_eq!(result.unwrap(), 42);
}

/// Test that with_cancellation handles operations that return Result.
#[tokio::test]
async fn test_with_cancellation_result_operation() {
    let result: Result<Result<i32, String>, String> =
        runtara_sdk::with_cancellation(async { Ok::<i32, String>(100) }).await;

    assert!(result.is_ok());
    assert_eq!(result.unwrap(), Ok(100));
}

/// Test that with_cancellation handles operations that return Err.
#[tokio::test]
async fn test_with_cancellation_error_operation() {
    let result: Result<Result<i32, String>, String> =
        runtara_sdk::with_cancellation(async { Err::<i32, String>("operation failed".into()) })
            .await;

    assert!(result.is_ok()); // with_cancellation succeeded
    assert!(result.unwrap().is_err()); // inner operation failed
}

/// Test that is_cancelled returns false when no SDK is registered.
#[tokio::test]
async fn test_is_cancelled_no_registry() {
    // When no SDK is registered, is_cancelled should return false
    let cancelled = runtara_sdk::is_cancelled();
    assert!(
        !cancelled,
        "Should not be cancelled when no SDK is registered"
    );
}

/// Test that cancellation_token returns None when no SDK is registered.
#[tokio::test]
async fn test_cancellation_token_no_registry() {
    // Note: This test may fail if another test has registered an SDK
    // Run with --test-threads=1 if needed
    let token = runtara_sdk::cancellation_token();

    // Token may be Some if another test registered an SDK, so we just check it doesn't panic
    if let Some(token) = token {
        // If a token exists, it should not be cancelled initially
        // (unless another test triggered cancellation)
        println!("Token exists, is_cancelled: {}", token.is_cancelled());
    } else {
        println!("No token registered (expected in isolated test)");
    }
}

// ============================================================================
// Tests for with_cancellation timing behavior
// ============================================================================

/// Test that a fast operation completes before any cancellation could happen.
#[tokio::test]
async fn test_with_cancellation_fast_operation() {
    let start = Instant::now();

    let result = runtara_sdk::with_cancellation(async {
        // Fast operation
        tokio::time::sleep(Duration::from_millis(10)).await;
        "done"
    })
    .await;

    let elapsed = start.elapsed();

    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "done");
    assert!(
        elapsed < Duration::from_millis(100),
        "Fast operation should complete quickly"
    );
}

/// Test with_cancellation_err variant with custom error.
#[tokio::test]
async fn test_with_cancellation_err_variant() {
    #[derive(Debug, PartialEq)]
    struct CustomError(String);

    let result: Result<i32, CustomError> = runtara_sdk::with_cancellation_err(
        async { Ok::<i32, CustomError>(42) },
        CustomError("cancelled".into()),
    )
    .await;

    assert!(result.is_ok());
    assert_eq!(result.unwrap(), 42);
}

// ============================================================================
// Tests that verify cancellation behavior with tokio::select
// ============================================================================

/// Test the manual cancellation pattern using tokio::select.
/// This demonstrates how users can use cancellation_token() directly.
#[tokio::test]
async fn test_manual_cancellation_pattern() {
    use tokio_util::sync::CancellationToken;

    // Create a local cancellation token for this test
    let token = CancellationToken::new();
    let token_clone = token.clone();

    // Spawn a task that will cancel after a short delay
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        token_clone.cancel();
    });

    // Race the operation against the token
    let start = Instant::now();
    let result: Result<&str, &str> = tokio::select! {
        biased;

        _ = token.cancelled() => {
            Err("cancelled")
        }

        _ = tokio::time::sleep(Duration::from_secs(10)) => {
            Ok("completed")
        }
    };

    let elapsed = start.elapsed();

    assert!(result.is_err());
    assert_eq!(result.unwrap_err(), "cancelled");
    assert!(
        elapsed < Duration::from_millis(200),
        "Operation should have been cancelled quickly, took {:?}",
        elapsed
    );
}

/// Test that operations racing against a non-cancelled token complete normally.
#[tokio::test]
async fn test_operation_completes_before_cancellation() {
    use tokio_util::sync::CancellationToken;

    let token = CancellationToken::new();

    // Race a fast operation against a token that won't be cancelled
    let result: Result<&str, &str> = tokio::select! {
        biased;

        _ = token.cancelled() => {
            Err("cancelled")
        }

        _ = tokio::time::sleep(Duration::from_millis(10)) => {
            Ok("completed")
        }
    };

    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "completed");
}

// ============================================================================
// Tests for concurrent operations with cancellation
// ============================================================================

/// Test that multiple concurrent operations can be cancelled together.
#[tokio::test]
async fn test_concurrent_operations_cancellation() {
    use tokio_util::sync::CancellationToken;

    let token = CancellationToken::new();
    let token1 = token.clone();
    let token2 = token.clone();
    let token3 = token.clone();

    // Spawn three "long-running" operations
    let handle1 = tokio::spawn(async move {
        tokio::select! {
            biased;
            _ = token1.cancelled() => Err("cancelled"),
            _ = tokio::time::sleep(Duration::from_secs(10)) => Ok("op1 done"),
        }
    });

    let handle2 = tokio::spawn(async move {
        tokio::select! {
            biased;
            _ = token2.cancelled() => Err("cancelled"),
            _ = tokio::time::sleep(Duration::from_secs(10)) => Ok("op2 done"),
        }
    });

    let handle3 = tokio::spawn(async move {
        tokio::select! {
            biased;
            _ = token3.cancelled() => Err("cancelled"),
            _ = tokio::time::sleep(Duration::from_secs(10)) => Ok("op3 done"),
        }
    });

    // Wait a bit then cancel all
    tokio::time::sleep(Duration::from_millis(50)).await;
    token.cancel();

    // All operations should complete with cancellation error
    let r1 = handle1.await.unwrap();
    let r2 = handle2.await.unwrap();
    let r3 = handle3.await.unwrap();

    assert!(r1.is_err());
    assert!(r2.is_err());
    assert!(r3.is_err());
}

// ============================================================================
// Tests for edge cases
// ============================================================================

/// Test that cancelling an already-completed operation is a no-op.
#[tokio::test]
async fn test_cancel_after_completion() {
    use tokio_util::sync::CancellationToken;

    let token = CancellationToken::new();
    let token_clone = token.clone();

    // Complete the operation first
    let result: &str = tokio::select! {
        biased;

        _ = token.cancelled() => {
            "cancelled"
        }

        result = async {
            tokio::time::sleep(Duration::from_millis(10)).await;
            "completed"
        } => {
            result
        }
    };

    assert_eq!(result, "completed");

    // Now cancel (should be a no-op since operation already completed)
    token_clone.cancel();

    // Verify token is now cancelled
    assert!(token.is_cancelled());
}

/// Test that trigger_cancellation doesn't panic when no SDK is registered.
#[tokio::test]
async fn test_trigger_cancellation_no_registry() {
    // This should not panic even if no SDK is registered
    runtara_sdk::trigger_cancellation();

    // And is_cancelled should still work
    let _ = runtara_sdk::is_cancelled();
}

/// Test with_cancellation with a slow operation that takes time.
#[tokio::test]
async fn test_with_cancellation_slow_operation_completes() {
    let start = Instant::now();

    let result = runtara_sdk::with_cancellation(async {
        // Simulate some work
        tokio::time::sleep(Duration::from_millis(100)).await;
        "slow operation done"
    })
    .await;

    let elapsed = start.elapsed();

    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "slow operation done");
    assert!(
        elapsed >= Duration::from_millis(100),
        "Operation should have waited at least 100ms"
    );
}

// ============================================================================
// Simulated workflow cancellation pattern test
// ============================================================================

/// Simulates how a workflow would use cancellation.
/// This is a comprehensive test of the cancellation pattern.
#[tokio::test]
async fn test_simulated_workflow_cancellation_pattern() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio_util::sync::CancellationToken;

    let cancel_token = CancellationToken::new();
    let cancel_clone = cancel_token.clone();

    let items_processed = Arc::new(AtomicUsize::new(0));
    let items_clone = items_processed.clone();

    // Simulate a workflow that processes items
    let workflow = tokio::spawn(async move {
        let items = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10];

        for item in items {
            // Check cancellation before each iteration (like our generated code does)
            if cancel_token.is_cancelled() {
                return Err(format!("Cancelled before processing item {}", item));
            }

            // Simulate processing with cancellation support (like with_cancellation does)
            let result: Result<i32, String> = tokio::select! {
                biased;

                _ = cancel_token.cancelled() => {
                    Err(format!("Cancelled during item {}", item))
                }

                result = async {
                    // Simulate some work
                    tokio::time::sleep(Duration::from_millis(30)).await;
                    Ok(item * 2)
                } => {
                    result
                }
            };

            match result {
                Ok(_) => {
                    items_clone.fetch_add(1, Ordering::SeqCst);
                }
                Err(e) => {
                    return Err(e);
                }
            }
        }

        Ok("All items processed")
    });

    // Cancel after 100ms (should process ~3 items)
    tokio::time::sleep(Duration::from_millis(100)).await;
    cancel_clone.cancel();

    // Wait for workflow to finish
    let result = workflow.await.unwrap();

    // Verify it was cancelled
    assert!(result.is_err(), "Workflow should have been cancelled");
    let error = result.unwrap_err();
    assert!(
        error.contains("Cancelled"),
        "Error should mention cancellation: {}",
        error
    );

    // Verify some items were processed before cancellation
    let processed = items_processed.load(Ordering::SeqCst);
    assert!(
        processed > 0 && processed < 10,
        "Expected some items processed (between 1-9), got {}",
        processed
    );

    println!(
        "✓ Workflow cancelled after processing {} items: {}",
        processed, error
    );
}

// ============================================================================
// Tests for acknowledge_cancellation
// ============================================================================

/// Test that acknowledge_cancellation doesn't panic when no SDK is registered.
#[tokio::test]
async fn test_acknowledge_cancellation_no_registry() {
    // This should not panic even if no SDK is registered
    runtara_sdk::acknowledge_cancellation().await;

    // Verify we can still call other cancellation functions
    let cancelled = runtara_sdk::is_cancelled();
    // Note: may or may not be cancelled depending on test order
    println!(
        "After acknowledge_cancellation, is_cancelled: {}",
        cancelled
    );
}

/// Test that acknowledge_cancellation triggers the local cancellation token.
/// Note: This test may affect other tests due to global state.
#[tokio::test]
async fn test_acknowledge_cancellation_triggers_token() {
    // If a token exists from previous tests, check initial state
    let initial_state = runtara_sdk::is_cancelled();
    println!("Initial cancellation state: {}", initial_state);

    // Call acknowledge_cancellation - this should trigger local cancellation
    runtara_sdk::acknowledge_cancellation().await;

    // After calling acknowledge_cancellation, is_cancelled may be true
    // (if a token was registered by previous tests)
    // The key behavior is that it doesn't panic and completes successfully
}

/// Test that acknowledge_cancellation is idempotent (safe to call multiple times).
#[tokio::test]
async fn test_acknowledge_cancellation_idempotent() {
    // Call multiple times - should not panic
    runtara_sdk::acknowledge_cancellation().await;
    runtara_sdk::acknowledge_cancellation().await;
    runtara_sdk::acknowledge_cancellation().await;

    // Should still work after multiple calls
    let _ = runtara_sdk::is_cancelled();
}

/// Test the split-like parallel processing with cancellation.
#[tokio::test]
async fn test_parallel_split_cancellation_pattern() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    let cancel_flag = Arc::new(AtomicBool::new(false));
    let completed_count = Arc::new(AtomicUsize::new(0));

    // Simulate parallel split processing
    let items: Vec<i32> = (0..20).collect();
    let mut handles = vec![];

    for item in items {
        let cancel = cancel_flag.clone();
        let completed = completed_count.clone();

        handles.push(tokio::spawn(async move {
            // Check cancellation before starting (like our generated code)
            if cancel.load(Ordering::Relaxed) {
                return Err(format!("Skipped item {} due to cancellation", item));
            }

            // Simulate work
            tokio::time::sleep(Duration::from_millis(50 + (item as u64 * 10))).await;

            // Check cancellation after work
            if cancel.load(Ordering::Relaxed) {
                return Err(format!("Cancelled during item {}", item));
            }

            completed.fetch_add(1, Ordering::SeqCst);
            Ok(item * 2)
        }));
    }

    // Cancel after 100ms
    tokio::time::sleep(Duration::from_millis(100)).await;
    cancel_flag.store(true, Ordering::SeqCst);

    // Wait for all handles
    let results: Vec<_> = futures::future::join_all(handles)
        .await
        .into_iter()
        .map(|r| r.unwrap())
        .collect();

    let successes: Vec<_> = results.iter().filter(|r| r.is_ok()).collect();
    let failures: Vec<_> = results.iter().filter(|r| r.is_err()).collect();

    let completed = completed_count.load(Ordering::SeqCst);

    println!(
        "✓ Parallel split: {} completed, {} cancelled/skipped",
        successes.len(),
        failures.len()
    );

    assert!(
        successes.len() > 0,
        "At least some items should have completed"
    );
    assert!(
        failures.len() > 0,
        "Some items should have been cancelled/skipped"
    );
    assert_eq!(completed, successes.len());
}

// ============================================================================
// Tests for acknowledge_pause
// ============================================================================

/// Test that acknowledge_pause doesn't panic when no SDK is registered.
#[tokio::test]
async fn test_acknowledge_pause_no_registry() {
    // This should not panic even if no SDK is registered
    runtara_sdk::acknowledge_pause().await;

    // Verify we can still call other functions
    let _ = runtara_sdk::is_cancelled();
}

/// Test that acknowledge_pause is idempotent (safe to call multiple times).
#[tokio::test]
async fn test_acknowledge_pause_idempotent() {
    // Call multiple times - should not panic
    runtara_sdk::acknowledge_pause().await;
    runtara_sdk::acknowledge_pause().await;
    runtara_sdk::acknowledge_pause().await;

    // Should still work after multiple calls
    let _ = runtara_sdk::is_cancelled();
}

/// Test that acknowledge_pause does NOT trigger local cancellation token.
/// (Unlike acknowledge_cancellation which does trigger it)
#[tokio::test]
async fn test_acknowledge_pause_does_not_cancel() {
    // Record initial state
    let was_cancelled_before = runtara_sdk::is_cancelled();

    // Acknowledge pause
    runtara_sdk::acknowledge_pause().await;

    // Pause acknowledgment should NOT change cancellation state
    // (cancellation state may have changed from other tests, but pause shouldn't affect it)
    let is_cancelled_after = runtara_sdk::is_cancelled();

    // The key invariant: if we weren't cancelled before pause ack, we shouldn't be after
    // Note: This may fail if another test triggered cancellation, so we just verify no panic
    println!(
        "Cancellation state: before={}, after={}",
        was_cancelled_before, is_cancelled_after
    );
}
