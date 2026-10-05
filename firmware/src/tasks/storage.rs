//! Storage: every write to the SPI flash. The UI queues channel, zone and preset changes and the settings
//! save in the store (crate::store); this task carries them out one by one, waiting for each program and
//! erase on a timer, so the UI never waits for the flash. The event log comes here too (milestone 6).

use crate::chan::STORAGE;
use crate::store::{STORE, WAKE};
use embassy_futures::select::{select, Either};
use ferrite_core::msg::StorageCmd;

#[embassy_executor::task]
pub async fn task() {
    let store = STORE.get().await.0;
    loop {
        while let Some(job) = store.next_job() {
            store.run(job).await;
        }
        match select(WAKE.wait(), STORAGE.receive()).await {
            Either::First(()) => {}
            Either::Second(StorageCmd::SaveSettings) => {} // the store's own save timer covers it
            Either::Second(StorageCmd::Log(_)) => {}       // event log ring: milestone 6
        }
    }
}
