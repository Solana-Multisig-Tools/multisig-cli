use std::thread;
use std::time::Duration;

pub fn retry_delay(attempt: u32) -> Duration {
    let ms = 200u64 * 2u64.saturating_pow(attempt);
    Duration::from_millis(ms.min(10_000))
}

pub fn sleep_before_retry(attempt: u32) {
    thread::sleep(retry_delay(attempt));
}
