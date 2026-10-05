//! Storage: the event log and other background writes (milestone 6). The settings and channel store is the UI
//! task's for now (crate::store: it reads records on demand and saves settings itself), so this task doesn't
//! touch the SPI flash yet; the SPI flash check is reported by the UI task when it mounts the store.

use crate::chan::STORAGE;
use ferrite_core::msg::StorageCmd;

#[embassy_executor::task]
pub async fn task() {
    loop {
        match STORAGE.receive().await {
            StorageCmd::SaveSettings => {} // the UI task saves the settings (store::Store::autosave)
            StorageCmd::Log(_) => {}       // event log ring: milestone 6
        }
    }
}
