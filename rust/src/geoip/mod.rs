pub mod dbip;
pub mod maxmind;
pub mod reader_slot;

use std::{net::IpAddr, sync::Arc};

use anyhow::Result;
use serde::Serialize;

use crate::config::GeoConfig;

#[derive(Debug, Default, Serialize)]
pub struct GeoLocation {
    pub ip: String,
    pub country_code: Option<String>,
    pub country: Option<String>,
    pub city: Option<String>,
    pub region: Option<String>,
    pub postal_code: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub time_zone: Option<String>,
    pub asn: Option<AsnInfo>,
    pub sources: GeoSources,
}

#[derive(Debug, Serialize)]
pub struct AsnInfo {
    pub number: Option<u32>,
    pub organization: Option<String>,
}

#[derive(Debug, Default, Serialize)]
pub struct GeoSources {
    pub location: Option<&'static str>,
    pub asn: Option<&'static str>,
}

pub trait GeoProviderAbs: Send + Sync {
    fn lookup(&self, ip: IpAddr) -> Result<Option<GeoLocation>>;
}

impl GeoLocation {
    /// MaxMind is preferred. Only empty attributes are backfilled from the
    /// next provider; individual ASN fields are never mixed across different
    /// ASN numbers or potentially unrelated organizations.
    fn fill_missing(&mut self, next: GeoLocation) {
        let mut new_location = false;
        // Refuse contradictory country records. A city/coordinate from another
        // country must not be combined with MaxMind's country.
        let compatible_country = match (&self.country_code, &next.country_code) {
            (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
            _ => true,
        };
        macro_rules! fill {
            ($field:ident) => {
                if self.$field.is_none() && next.$field.is_some() {
                    self.$field = next.$field;
                    new_location = true;
                }
            };
        }
        if compatible_country {
            fill!(country_code);
            fill!(country);
            fill!(city);
            fill!(region);
            fill!(postal_code);
            fill!(latitude);
            fill!(longitude);
            fill!(time_zone);
        }
        if new_location {
            self.sources.location = match (self.sources.location, next.sources.location) {
                (Some("maxmind"), Some("dbip")) => Some("maxmind+dbip"),
                (existing, Some("dbip")) if existing != Some("dbip") => Some("dbip"),
                (existing, _) => existing,
            };
        }

        if let Some(incoming) = next.asn {
            match self.asn.as_mut() {
                None => {
                    self.asn = Some(incoming);
                    self.sources.asn = next.sources.asn;
                }
                Some(existing) if existing.number.is_some() && existing.number == incoming.number => {
                    if existing.organization.is_none() && incoming.organization.is_some() {
                        existing.organization = incoming.organization;
                        if self.sources.asn == Some("maxmind") && next.sources.asn == Some("dbip") {
                            self.sources.asn = Some("maxmind+dbip");
                        }
                    }
                }
                _ => {}
            }
        }
    }

    pub fn uses_dbip(&self) -> bool {
        self.sources.location.is_some_and(|s| s.contains("dbip"))
            || self.sources.asn.is_some_and(|s| s.contains("dbip"))
    }
}

pub struct GeoService {
    providers: Vec<Arc<dyn GeoProviderAbs>>,
    maxmind_readers: Option<Arc<maxmind::MaxMindReaders>>,
    dbip_readers: Option<Arc<dbip::DbIpReaders>>,
}

impl GeoService {
    pub fn load(config: &GeoConfig) -> Result<Self> {
        let mut providers: Vec<Arc<dyn GeoProviderAbs>> = Vec::new();
        let mut maxmind_readers = None;
        if let Some(provider) = maxmind::MaxMindProvider::load(&config.maxmind)? {
            maxmind_readers = Some(provider.live_readers());
            providers.push(Arc::new(provider));
        }

        let mut dbip_readers = None;
        if let Some(provider) = dbip::DbIpProvider::load(&config.dbip)? {
            dbip_readers = Some(provider.live_readers());
            providers.push(Arc::new(provider));
        }
        Ok(Self { providers, maxmind_readers, dbip_readers })
    }

    pub fn maxmind_live_readers(&self) -> Option<Arc<maxmind::MaxMindReaders>> {
        self.maxmind_readers.clone()
    }

    pub fn dbip_live_readers(&self) -> Option<Arc<dbip::DbIpReaders>> {
        self.dbip_readers.clone()
    }

    pub fn has_providers(&self) -> bool {
        !self.providers.is_empty()
    }

    pub fn lookup(&self, ip: IpAddr) -> Result<Option<GeoLocation>> {
        let mut result: Option<GeoLocation> = None;
        for provider in &self.providers {
            if let Some(location) = provider.lookup(ip)? {
                match &mut result {
                    Some(existing) => existing.fill_missing(location),
                    None => result = Some(location),
                }
            }
        }
        Ok(result)
    }
}

#[cfg(test)]
mod merge_tests {
    use super::*;

    #[test]
    fn maxmind_keeps_priority_and_dbip_fills_missing_city() {
        let mut maxmind = GeoLocation {
            ip: "8.8.8.8".into(), country: Some("United States".into()),
            country_code: Some("US".into()),
            sources: GeoSources { location: Some("maxmind"), ..GeoSources::default() },
            ..GeoLocation::default()
        };
        let dbip = GeoLocation {
            ip: "8.8.8.8".into(), country: Some("USA".into()),
            city: Some("Mountain View".into()),
            sources: GeoSources { location: Some("dbip"), ..GeoSources::default() },
            ..GeoLocation::default()
        };
        maxmind.fill_missing(dbip);
        assert_eq!(maxmind.country.as_deref(), Some("United States"));
        assert_eq!(maxmind.city.as_deref(), Some("Mountain View"));
        assert_eq!(maxmind.sources.location, Some("maxmind+dbip"));
        assert!(maxmind.uses_dbip());
    }

    #[test]
    fn inconsistent_country_is_not_mixed_with_fallback_city() {
        let mut first = GeoLocation {
            country_code: Some("TR".into()),
            country: Some("Türkiye".into()),
            sources: GeoSources { location: Some("maxmind"), ..GeoSources::default() },
            ..GeoLocation::default()
        };
        first.fill_missing(GeoLocation {
            country_code: Some("US".into()), city: Some("Boston".into()),
            sources: GeoSources { location: Some("dbip"), ..GeoSources::default() },
            ..GeoLocation::default()
        });
        assert!(first.city.is_none());
        assert_eq!(first.sources.location, Some("maxmind"));
    }

    #[test]
    fn different_asn_numbers_are_not_mixed() {
        let mut first = GeoLocation { asn: Some(AsnInfo { number: Some(123), organization: None }),
            sources: GeoSources { asn: Some("maxmind"), ..GeoSources::default() }, ..GeoLocation::default() };
        let next = GeoLocation { asn: Some(AsnInfo { number: Some(456), organization: Some("Different".into()) }),
            sources: GeoSources { asn: Some("dbip"), ..GeoSources::default() }, ..GeoLocation::default() };
        first.fill_missing(next);
        assert_eq!(first.asn.unwrap().organization, None);
        assert_eq!(first.sources.asn, Some("maxmind"));
    }
}
