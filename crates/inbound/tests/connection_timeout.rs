use std::time::Duration;

use inbound::timeout::ConnectionTimeout;
use tokio::io::AsyncReadExt;

#[tokio::test(start_paused = true)]
async fn idle_timeout_fires_when_no_data_arrives() {
    let mock = tokio_test::io::Builder::new()
        .wait(Duration::from_secs(2))
        .build();
    let (mut wrapped, _handle) =
        ConnectionTimeout::new(mock, Duration::from_millis(500), Duration::from_secs(10));

    let mut buf = [0u8; 16];
    let err = wrapped
        .read(&mut buf)
        .await
        .expect_err("idle timeout should fire");

    assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
}

#[tokio::test(start_paused = true)]
async fn idle_timeout_resets_when_data_arrives_in_time() {
    let mock = tokio_test::io::Builder::new()
        .wait(Duration::from_millis(300))
        .read(b"a")
        .wait(Duration::from_millis(300))
        .read(b"b")
        .build();
    let (mut wrapped, handle) =
        ConnectionTimeout::new(mock, Duration::from_millis(500), Duration::from_secs(10));

    let mut buf = [0u8; 16];
    let n1 = wrapped.read(&mut buf).await.expect("first read should succeed");
    assert_eq!(&buf[..n1], b"a");

    // Pretend the tiny request was fully handled, so the next read goes
    // through the idle-waiting path again instead of the header path.
    handle.end_request();

    let n2 = wrapped.read(&mut buf).await.expect("second read should succeed");
    assert_eq!(&buf[..n2], b"b");
}

#[tokio::test(start_paused = true)]
async fn header_deadline_is_not_extended_by_trickling_data() {
    let mock = tokio_test::io::Builder::new()
        .read(b"G")
        .wait(Duration::from_millis(80))
        .read(b"E")
        .wait(Duration::from_millis(80))
        .read(b"T")
        .wait(Duration::from_millis(80))
        .build();
    let (mut wrapped, _handle) =
        ConnectionTimeout::new(mock, Duration::from_secs(10), Duration::from_millis(200));

    let mut buf = [0u8; 1];
    let n = wrapped
        .read(&mut buf)
        .await
        .expect("first byte should start the header-reading phase");
    assert!(n > 0);

    // Each subsequent byte arrives inside its own 80ms gap (which would
    // reset an inactivity timer), but the three gaps add up to 240ms,
    // past the fixed 200ms deadline computed from the first byte.
    let mut saw_timeout = false;
    for _ in 0..3 {
        match wrapped.read(&mut buf).await {
            Ok(_) => {}
            Err(err) => {
                assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
                saw_timeout = true;
                break;
            }
        }
    }

    assert!(saw_timeout, "header deadline should have fired");
}

#[tokio::test(start_paused = true)]
async fn processing_phase_disables_all_timeouts() {
    let mock = tokio_test::io::Builder::new()
        .read(b"x")
        .wait(Duration::from_secs(5))
        .read(b"y")
        .build();
    let (mut wrapped, handle) =
        ConnectionTimeout::new(mock, Duration::from_millis(100), Duration::from_millis(100));

    let mut buf = [0u8; 1];
    let n = wrapped.read(&mut buf).await.expect("first byte should arrive");
    assert!(n > 0);

    // Headers are "done"; no timeout should apply while a request runs.
    handle.begin_processing();

    let n = wrapped
        .read(&mut buf)
        .await
        .expect("read during processing must not time out");
    assert_eq!(&buf[..n], b"y");
}
