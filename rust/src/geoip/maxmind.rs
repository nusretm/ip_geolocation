pub mod download;
pub mod schedule;
pub mod update;

use std::net::IpAddr;

use anyhow::{bail, Context, Result};
use maxminddb::Reader;
use serde_json::Value;

use super::{AsnInfo, GeoLocation, GeoProviderAbs};
use crate::config::MaxMindConfig;

pub struct MaxMindProvider {
    city: Option<Reader<Vec<u8>>>,
    country: Option<Reader<Vec<u8>>>,
    asn: Option<Reader<Vec<u8>>>,
}

fn open_database(path: &std::path::Path, edition: &str) -> Result<Reader<Vec<u8>>> {
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
        Ok(Some(Self { city, country, asn }))
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
        if let Some(reader) = &self.city {
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
        if let Some(reader) = &self.country {
            if let Some(record) = reader.lookup(ip)?.decode::<Value>()? {
                has_data = true;
                fill_country(&mut location, &record);
                location.sources.location = Some("maxmind");
            }
        }
        if let Some(reader) = &self.asn {
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
