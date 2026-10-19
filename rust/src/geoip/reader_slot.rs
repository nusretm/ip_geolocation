use std::sync::{Arc, RwLock};

use maxminddb::Reader;

pub type DatabaseFingerprint = [u8; 32];

struct ActiveReader {
    reader: Arc<Reader<Vec<u8>>>,
    fingerprint: DatabaseFingerprint,
}

/// A short-lived, per-database pointer lock. Queries clone the Arc and release
/// this lock before decoding; swapping never waits for a query to finish.
/// The fingerprint identifies database content, not merely its build timestamp.
#[derive(Default)]
pub struct ReaderSlot {
    active: RwLock<Option<ActiveReader>>,
}

impl ReaderSlot {
    pub fn new(reader: Option<(Reader<Vec<u8>>, DatabaseFingerprint)>) -> Self {
        Self {
            active: RwLock::new(reader.map(|(reader, fingerprint)| ActiveReader {
                reader: Arc::new(reader),
                fingerprint,
            })),
        }
    }

    pub fn snapshot(&self) -> Option<Arc<Reader<Vec<u8>>>> {
        self.active
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .map(|value| value.reader.clone())
    }

    pub fn fingerprint(&self) -> Option<DatabaseFingerprint> {
        self.active
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .map(|value| value.fingerprint)
    }

    pub fn activate(&self, reader: Reader<Vec<u8>>, fingerprint: DatabaseFingerprint) {
        // The swap consists only of an Arc pointer assignment. Readers already
        // holding the previous Arc continue decoding from the old database.
        *self
            .active
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(ActiveReader {
                reader: Arc::new(reader),
                fingerprint,
            });
    }

    pub fn build_epoch(&self) -> Option<u64> {
        self.snapshot().map(|reader| reader.metadata.build_epoch)
    }
}
