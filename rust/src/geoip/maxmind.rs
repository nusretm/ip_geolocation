pub mod download;
pub mod schedule;
pub mod update;

use std::{net::IpAddr, path::Path, sync::Arc};

use anyhow::{bail, Context, Result};
use maxminddb::Reader;
use serde_json::Value;

use super::{reader_slot::ReaderSlot, AsnInfo, GeoLocation, GeoProviderAbs};
use schedule::DatabaseKind;
use crate::config::MaxMindConfig;

/// Shared with the MaxMind updater; City, Country and ASN have independent
/// reader pointers. No request owns a filesystem handle or holds a lock while
/// decoding MMDB values.
#[derive(Default)]
pub struct MaxMindReaders {
    city: ReaderSlot,
    country: ReaderSlot,
    asn: ReaderSlot,
}

impl MaxMindReaders {
    fn slot(&self, kind: DatabaseKind) -> &ReaderSlot {
        match kind {
            DatabaseKind::City => &self.city,
            DatabaseKind::Country => &self.country,
            DatabaseKind::Asn => &self.asn,
        }
    }

    pub fn activate(&self, kind: DatabaseKind, reader: Reader<Vec<u8>>) -> Result<()> {
        if !reader.metadata.database_type.contains(kind.expected_type()) {
            bail!("Refusing to activate an MMDB with the wrong database type");
        }
        self.slot(kind).activate(reader);
        Ok(())
    }

    pub fn active_build_epoch(&self, kind: DatabaseKind) -> Option<u64> {
        self.slot(kind).build_epoch()
    }

    pub fn reload_if_changed(&self, kind: DatabaseKind, path: &Path) -> Result<bool> {
        if !path.exists() {
            return Ok(false);
        }
        let reader = open_database(path, kind.expected_type())?;
        let epoch = reader.metadata.build_epoch;
        if epoch == 0 {
            bail!("Refusing to reload an MMDB without a valid build epoch");
        }
        if self.slot(kind).build_epoch() == Some(epoch) {
            return Ok(false);
        }
        self.activate(kind, reader)?;
        Ok(true)
    }

    fn snapshot(&self, kind: DatabaseKind) -> Option<Arc<Reader<Vec<u8>>>> {
        self.slot(kind).snapshot()
    }
}

pub struct MaxMindProvider {
    readers: Arc<MaxMindReaders>,
}

fn open_database(path: &Path, edition: &str) -> Result<Reader<Vec<u8>>> {
    let reader = Reader::open_readfile(path)
        .with_context(|| format!("Unable to read {edition} MMDB: {}", path.display()))?;
    let database_type = &reader.metadata.database_type;
    if !database_type.contains(edition) {
        bail!("Expected {edition} database at {}, found {database_type}", path.display());
    }
    Ok(reader)
}

impl MaxMindProvider {
    pub fn load(config: &MaxMindConfig) -> Result<Option<Self>> {
        let city = config.city.as_deref()
            .map(|path| open_database(path, "City")).transpose()?;
        let country = config.country.as_deref()
            .map(|path| open_database(path, "Country")).transpose()?;
        let asn = config.asn.as_deref()
            .map(|path| open_database(path, "ASN")).transpose()?;
        if city.is_none() && country.is_none() && asn.is_none() {
            return Ok(None);
        }
        Ok(Some(Self {
            readers: Arc::new(MaxMindReaders {
                city: ReaderSlot::new(city),
                country: ReaderSlot::new(country),
                asn: ReaderSlot::new(asn),
            }),
        }))
    }

    pub fn live_readers(&self) -> Arc<MaxMindReaders> {
        self.readers.clone()
    }
}

fn string_at(record: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .try_fold(record, |value, key| value.get(*key))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn fill_country(location: &mut GeoLocation, record: &Value) {
    if location.country_code.is_none() {
        location.country_code = string_at(record, &["country", "iso_code"]);
    }
    if location.country.is_none() {
        location.country = string_at(record, &["country", "names", "en"]);
    }
}

impl GeoProviderAbs for MaxMindProvider {
    fn lookup(&self, ip: IpAddr) -> Result<Option<GeoLocation>> {
        let mut location = GeoLocation { ip: ip.to_string(), ..GeoLocation::default() };
        let mut has_data = false;
        if let Some(reader) = self.readers.snapshot(DatabaseKind::City) {
            if let Some(record) = reader.lookup(ip)?.decode::<Value>()? {
                has_data = true;
                fill_country(&mut location, &record);
                location.city = string_at(&record, &["city", "names", "en"]);
                location.region = record.get("subdivisions")
                    .and_then(Value::as_array)
                    .and_then(|subdivisions| subdivisions.last())
                    .and_then(|subdivision| string_at(subdivision, &["names", "en"]));
                location.postal_code = string_at(&record, &["postal", "code"]);
                location.latitude = record.pointer("/location/latitude").and_then(Value::as_f64);
                location.longitude = record.pointer("/location/longitude").and_then(Value::as_f64);
                location.time_zone = string_at(&record, &["location", "time_zone"]);
                location.sources.location = Some("maxmind");
            }
        }
        if let Some(reader) = self.readers.snapshot(DatabaseKind::Country) {
            if let Some(record) = reader.lookup(ip)?.decode::<Value>()? {
                has_data = true;
                fill_country(&mut location, &record);
                location.sources.location = Some("maxmind");
            }
        }
        if let Some(reader) = self.readers.snapshot(DatabaseKind::Asn) {
            if let Some(record) = reader.lookup(ip)?.decode::<Value>()? {
                has_data = true;
                location.asn = Some(AsnInfo {
                    number: record.get("autonomous_system_number").and_then(Value::as_u64)
                        .and_then(|number| u32::try_from(number).ok()),
                    organization: string_at(&record, &["autonomous_system_organization"]),
                });
                location.sources.asn = Some("maxmind");
            }
        }
        Ok(if has_data { Some(location) } else { None })
    }
}
