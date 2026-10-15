//! Count only the CPU consumed while this future is being polled. Do not charge
//! unrelated Tokio tasks or time waiting for the database to this request.
use std::future::{poll_fn, Future};

pub(super) fn thread_cpu_nanoseconds() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let mut time = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: clock_gettime writes to a valid timespec, with no retained pointer.
        if unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut time) } != 0 {
            return None;
        }
        u64::try_from(time.tv_sec)
            .ok()?
            .checked_mul(1_000_000_000)?
            .checked_add(u64::try_from(time.tv_nsec).ok()?)
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

pub(super) async fn with_poll_cpu<F: Future>(future: F) -> (F::Output, Option<f64>) {
    let mut future = std::pin::pin!(future);
    let mut nanoseconds = Some(0u64);
    let result = poll_fn(|context| {
        let before = thread_cpu_nanoseconds();
        let result = future.as_mut().poll(context);
        let after = thread_cpu_nanoseconds();
        nanoseconds = nanoseconds
            .zip(before.zip(after))
            .and_then(|(total, (before, after))| total.checked_add(after.checked_sub(before)?));
        result
    })
    .await;
    (
        result,
        nanoseconds.map(|value| value as f64 / 1_000_000_000.0),
    )
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn poll_cpu_counts_work_without_including_pending_time() {
    let (value, cpu) = with_poll_cpu(async {
        let mut value = 0u64;
        for i in 0..100_000 {
            value = std::hint::black_box(value.wrapping_add(i));
        }
        tokio::task::yield_now().await;
        value
    })
    .await;
    assert_eq!(value, 4_999_950_000);
    assert!(cpu.is_some_and(|seconds| seconds > 0.0));
}
