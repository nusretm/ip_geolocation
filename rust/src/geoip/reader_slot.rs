use std::sync::{Arc, RwLock};

use maxminddb::Reader;

/// A short-lived, per-database pointer lock. Queries clone the Arc and release
/// this lock before decoding; swapping never waits for a query to finish.
#[derive(Default)]
pub struct ReaderSlot {
    active: RwLock<Option<Arc<Reader<Vec<u8>>>>>,
}

impl ReaderSlot {
    pub fn new(reader: Option<Reader<Vec<u8>>>) -> Self {
        Self {
            active: RwLock::new(reader.map(Arc::new)),
        }
    }

    pub fn snapshot(&self) -> Option<Arc<Reader<Vec<u8>>>> {
        self.active
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn activate(&self, reader: Reader<Vec<u8>>) {
        // The swap consists only of an Arc pointer assignment. Readers already
        // holding the previous Arc continue decoding from the old database.
        *self
            .active
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Arc::new(reader));
    }

    pub fn build_epoch(&self) -> Option<u64> {
        self.snapshot().map(|reader| reader.metadata.build_epoch)
    }
}
