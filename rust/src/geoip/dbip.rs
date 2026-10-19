pub mod download;
pub mod update;

use std::{net::IpAddr, path::Path, sync::Arc};

use anyhow::{bail, Context, Result};
use maxminddb::Reader;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{
    reader_slot::{DatabaseFingerprint, ReaderSlot},
    AsnInfo, GeoLocation, GeoProviderAbs,
};
use crate::config::DbIpConfig;

/// DB-IP has its own database/edition semantics and download cadence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DatabaseKind {
    City,
    Country,
    Asn,
}

impl DatabaseKind {
    pub fn name(self) -> &'static str {
        match self { Self::City => "city", Self::Country => "country", Self::Asn => "asn" }
    }

    pub fn target_name(self) -> &'static str {
        match self { Self::City => "dbip-city-lite", Self::Country => "dbip-country-lite", Self::Asn => "dbip-asn-lite" }
    }

    pub fn configured<'a>(self, config: &'a DbIpConfig) -> Option<&'a Path> {
        match self {
            Self::City => config.city.as_deref(),
            Self::Country => config.country.as_deref(),
            Self::Asn => config.asn.as_deref(),
        }
    }
}

pub(crate) fn reader_from_bytes(bytes: Vec<u8>, kind: DatabaseKind) -> Result<(Reader<Vec<u8>>, DatabaseFingerprint)> {
    let fingerprint = Sha256::digest(&bytes).into();
    let reader = Reader::from_source(bytes).context("Invalid DB-IP MMDB")?;
    let database_type = reader.metadata.database_type.to_ascii_lowercase();
    // DB-IP Lite MMDBs use the standard MaxMind-compatible record structure,
    // but their metadata must identify DB-IP and the selected edition.
    if !(database_type.contains("dbip") || database_type.contains("db-ip"))
        || !database_type.contains(kind.name())
    {
        bail!("DB-IP {} database type mismatch: {}", kind.name(), reader.metadata.database_type);
    }
    if reader.metadata.build_epoch == 0 {
        bail!("DB-IP database has no valid build epoch");
    }
    Ok((reader, fingerprint))
}

fn open_database(path: &Path, kind: DatabaseKind) -> Result<(Reader<Vec<u8>>, DatabaseFingerprint)> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("Could not read DB-IP {} MMDB: {}", kind.name(), path.display()))?;
    reader_from_bytes(bytes, kind)
}

#[derive(Default)]
pub struct DbIpReaders {
    city: ReaderSlot,
    country: ReaderSlot,
    asn: ReaderSlot,
}

impl DbIpReaders {
    fn slot(&self, kind: DatabaseKind) -> &ReaderSlot {
        match kind {
            DatabaseKind::City => &self.city,
            DatabaseKind::Country => &self.country,
            DatabaseKind::Asn => &self.asn,
        }
    }

    pub fn activate(&self, kind: DatabaseKind, reader: Reader<Vec<u8>>, fingerprint: DatabaseFingerprint) -> Result<()> {
        let database_type = reader.metadata.database_type.to_ascii_lowercase();
        if !(database_type.contains("dbip") || database_type.contains("db-ip"))
            || !database_type.contains(kind.name())
        {
            bail!("Refusing incompatible DB-IP {} database", kind.name());
        }
        self.slot(kind).activate(reader, fingerprint);
        Ok(())
    }

    pub fn reload_if_changed(&self, kind: DatabaseKind, path: &Path) -> Result<bool> {
        if !path.exists() { return Ok(false); }
        let (reader, fingerprint) = open_database(path, kind)?;
        if self.slot(kind).fingerprint() == Some(fingerprint) {
            return Ok(false);
        }
        self.activate(kind, reader, fingerprint)?;
        Ok(true)
    }

    fn snapshot(&self, kind: DatabaseKind) -> Option<Arc<Reader<Vec<u8>>>> {
        self.slot(kind).snapshot()
    }
}

pub struct DbIpProvider {
    readers: Arc<DbIpReaders>,
}

impl DbIpProvider {
    pub fn load(config: &DbIpConfig) -> Result<Option<Self>> {
        let city = config.city.as_deref().map(|path| open_database(path, DatabaseKind::City)).transpose()?;
        let country = config.country.as_deref().map(|path| open_database(path, DatabaseKind::Country)).transpose()?;
        let asn = config.asn.as_deref().map(|path| open_database(path, DatabaseKind::Asn)).transpose()?;
        if city.is_none() && country.is_none() && asn.is_none() { return Ok(None); }
        Ok(Some(Self {
            readers: Arc::new(DbIpReaders {
                city: ReaderSlot::new(city),
                country: ReaderSlot::new(country),
                asn: ReaderSlot::new(asn),
            }),
        }))
    }

    pub fn live_readers(&self) -> Arc<DbIpReaders> { self.readers.clone() }
}

fn string_at(record: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().try_fold(record, |value, key| value.get(*key))
        .and_then(Value::as_str).map(str::to_owned)
}

fn fill_country(location: &mut GeoLocation, record: &Value) {
    if location.country_code.is_none() {
        location.country_code = string_at(record, &["country", "iso_code"]);
    }
    if location.country.is_none() {
        location.country = string_at(record, &["country", "names", "en"]);
    }
}

impl GeoProviderAbs for DbIpProvider {
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
                    .and_then(|items| items.last())
                    .and_then(|last| string_at(last, &["names", "en"]));
                location.latitude = record.pointer("/location/latitude").and_then(Value::as_f64);
                location.longitude = record.pointer("/location/longitude").and_then(Value::as_f64);
                location.sources.location = Some("dbip");
            }
        }
        if let Some(reader) = self.readers.snapshot(DatabaseKind::Country) {
            if let Some(record) = reader.lookup(ip)?.decode::<Value>()? {
                has_data = true;
                fill_country(&mut location, &record);
                location.sources.location = Some("dbip");
            }
        }
        if let Some(reader) = self.readers.snapshot(DatabaseKind::Asn) {
            if let Some(record) = reader.lookup(ip)?.decode::<Value>()? {
                let number = record.get("autonomous_system_number")
                    .or_else(|| record.get("asn"))
                    .and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok());
                let organization = string_at(&record, &["autonomous_system_organization"])
                    .or_else(|| string_at(&record, &["aso"]));
                if number.is_some() || organization.is_some() {
                    has_data = true;
                    location.asn = Some(AsnInfo { number, organization });
                    location.sources.asn = Some("dbip");
                }
            }
        }
        Ok(if has_data { Some(location) } else { None })
    }
}
