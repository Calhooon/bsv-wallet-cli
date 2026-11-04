//! The held calls' own edge (#534): a kept place whose row's read fails is
//! logged by name and freed at its window's end regardless, the window
//! being the bound.

use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::*;
use crate::server::owner_door_tests::wallet;

/// The log's lines, as the served build writes them.
#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<u8>>>);

impl Write for Log {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Log {
    type Writer = Self;

    fn make_writer(&'a self) -> Self {
        self.clone()
    }
}

#[tokio::test]
async fn a_failed_read_keeps_the_place_until_its_window_ends_and_says_so() {
    let log = Log::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(log.clone())
        .with_ansi(false)
        .finish();
    let _logging = tracing::subscriber::set_default(subscriber);
    let (_dir, wallet) = wallet().await;
    let held = Held::with_window(Duration::from_secs(3600));
    let now = owner_door::now().unwrap();
    let mut place = held.place("page.example").unwrap();
    place.passed = true;
    drop(place);
    let originator = "page.example".to_string();
    let until = now + 2;
    held.map().kept.insert(7, Kept { originator, until });
    wallet.storage().pool().close().await;

    free_if_gone(&held, &wallet, 7).await;

    let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    let words = "browser gate: the row's read failed; its place is freed at its window's end";
    assert!(
        text.contains(words) && text.contains("pending_id=7"),
        "{text}"
    );
    assert_eq!(held.kept(), [7], "kept inside its window");
    assert_eq!(held.counts("page.example"), (1, 1));
    while owner_door::now().unwrap() < until {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(held.kept().is_empty(), "freed at its window's end");
    assert_eq!(held.counts("page.example"), (0, 0));
}
