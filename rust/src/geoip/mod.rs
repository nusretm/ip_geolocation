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
    /// A GeoIP provider's detailed location is a coherent record, not a bag
    /// of independent fields. Mixing a fallback city with a different source's
    /// latitude, longitude and time zone can yield impossible combinations.
    fn location_detail_rank(&self) -> u8 {
        if self.city.is_some() {
            4
        } else if self.region.is_some() || self.postal_code.is_some() {
            3
        } else if self.latitude.is_some() && self.longitude.is_some() {
            2
        } else if self.latitude.is_some() || self.longitude.is_some() || self.time_zone.is_some() {
            1
        } else {
            0
        }
    }

    fn fill_missing(&mut self, next: GeoLocation) {
        // Explicitly reject different countries, or an unknown fallback
        // country when the preferred provider already identified one.
        let compatible_country = match (&self.country_code, &next.country_code) {
            (Some(first), Some(second)) => first.eq_ignore_ascii_case(second),
            (Some(_), None) => false,
            (None, _) => match (&self.country, &next.country) {
                (Some(first), Some(second)) => first.eq_ignore_ascii_case(second),
                _ => true,
            },
        };

        if compatible_country {
            let mut used_fallback = false;

            // Country identity can be filled independently of the city-level
            // geolocation, but never overwritten when already known.
            if self.country_code.is_none() && next.country_code.is_some() {
                self.country_code = next.country_code.clone();
                used_fallback = true;
            }
            if self.country.is_none() && next.country.is_some() {
                self.country = next.country.clone();
                used_fallback = true;
            }

            // Prefer MaxMind at equal precision. If DB-IP has a more specific
            // location (e.g. City vs MaxMind's country-centroid coordinates),
            // replace the *entire* detailed record. Do not keep stale
            // coordinates, postal code or time zone from the previous source.
            if next.location_detail_rank() > self.location_detail_rank() {
                self.city = next.city;
                self.region = next.region;
                self.postal_code = next.postal_code;
                self.latitude = next.latitude;
                self.longitude = next.longitude;
                self.time_zone = next.time_zone;
                used_fallback = true;
            }

            if used_fallback {
                self.sources.location = match (self.sources.location, next.sources.location) {
                    (Some("maxmind"), Some("dbip")) => Some("maxmind+dbip"),
                    (_, Some(source)) => Some(source),
                    (existing, None) => existing,
                };
            }
        }

        // ASN is independent of geographic position. Only extend an existing
        // ASN organization if both providers agree on its ASN number.
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
    fn more_specific_dbip_city_replaces_entire_maxmind_centroid_profile() {
        // This reproduces the real Windows service observation for 8.8.8.8:
        // combining Mountain View with a MaxMind US centroid yielded
        // coordinates far away from California and a Chicago time zone.
        let mut maxmind = GeoLocation {
            ip: "8.8.8.8".into(),
            country_code: Some("US".into()),
            country: Some("United States".into()),
            latitude: Some(37.751),
            longitude: Some(-97.822),
            time_zone: Some("America/Chicago".into()),
            postal_code: None,
            sources: GeoSources { location: Some("maxmind"), ..GeoSources::default() },
            ..GeoLocation::default()
        };
        let dbip = GeoLocation {
            ip: "8.8.8.8".into(),
            country_code: Some("US".into()),
            country: Some("United States".into()),
            city: Some("Mountain View".into()),
            region: Some("California".into()),
            latitude: Some(37.422),
            longitude: Some(-122.085),
            time_zone: None,
            sources: GeoSources { location: Some("dbip"), ..GeoSources::default() },
            ..GeoLocation::default()
        };

        maxmind.fill_missing(dbip);
        assert_eq!(maxmind.city.as_deref(), Some("Mountain View"));
        assert_eq!(maxmind.region.as_deref(), Some("California"));
        assert_eq!(maxmind.latitude, Some(37.422));
        assert_eq!(maxmind.longitude, Some(-122.085));
        assert_eq!(maxmind.time_zone, None); // No stale Chicago time zone.
        assert_eq!(maxmind.country_code.as_deref(), Some("US"));
        assert_eq!(maxmind.sources.location, Some("maxmind+dbip"));
    }

    #[test]
    fn maxmind_city_remains_authoritative_even_if_dbip_has_missing_coordinates() {
        let mut maxmind = GeoLocation {
            country_code: Some("US".into()),
            country: Some("United States".into()),
            city: Some("Seattle".into()),
            region: Some("Washington".into()),
            postal_code: Some("98101".into()),
            time_zone: Some("America/Los_Angeles".into()),
            latitude: None,
            longitude: None,
            sources: GeoSources { location: Some("maxmind"), ..GeoSources::default() },
            ..GeoLocation::default()
        };
        let dbip = GeoLocation {
            country_code: Some("US".into()),
            city: Some("Mountain View".into()),
            region: Some("California".into()),
            latitude: Some(37.422),
            longitude: Some(-122.085),
            sources: GeoSources { location: Some("dbip"), ..GeoSources::default() },
            ..GeoLocation::default()
        };
        maxmind.fill_missing(dbip);
        assert_eq!(maxmind.city.as_deref(), Some("Seattle"));
        assert_eq!(maxmind.region.as_deref(), Some("Washington"));
        assert_eq!(maxmind.postal_code.as_deref(), Some("98101"));
        assert_eq!(maxmind.latitude, None); // Do not attach a different city's coordinates.
        assert_eq!(maxmind.longitude, None);
        assert_eq!(maxmind.time_zone.as_deref(), Some("America/Los_Angeles"));
        assert_eq!(maxmind.sources.location, Some("maxmind"));
    }

    #[test]
    fn inconsistent_country_cannot_replace_location_profile_even_if_more_precise() {
        let mut maxmind = GeoLocation {
            country_code: Some("TR".into()), country: Some("Türkiye".into()),
            latitude: Some(39.0), longitude: Some(35.0),
            sources: GeoSources { location: Some("maxmind"), ..GeoSources::default() },
            ..GeoLocation::default()
        };
        let dbip = GeoLocation {
            country_code: Some("US".into()), country: Some("United States".into()),
            city: Some("Mountain View".into()), region: Some("California".into()),
            latitude: Some(37.422), longitude: Some(-122.085),
            sources: GeoSources { location: Some("dbip"), ..GeoSources::default() },
            ..GeoLocation::default()
        };
        maxmind.fill_missing(dbip);
        assert!(maxmind.city.is_none());
        assert_eq!(maxmind.latitude, Some(39.0));
        assert_eq!(maxmind.longitude, Some(35.0));
        assert_eq!(maxmind.sources.location, Some("maxmind"));
    }

    #[test]
    fn country_only_maxmind_can_gain_complete_dbip_city_record() {
        let mut maxmind = GeoLocation {
            country_code: Some("US".into()), country: Some("United States".into()),
            sources: GeoSources { location: Some("maxmind"), ..GeoSources::default() },
            ..GeoLocation::default()
        };
        let dbip = GeoLocation {
            country_code: Some("US".into()), city: Some("Mountain View".into()),
            region: Some("California".into()),
            latitude: Some(37.422), longitude: Some(-122.085),
            sources: GeoSources { location: Some("dbip"), ..GeoSources::default() },
            ..GeoLocation::default()
        };
        maxmind.fill_missing(dbip);
        assert_eq!(maxmind.city.as_deref(), Some("Mountain View"));
        assert_eq!(maxmind.latitude, Some(37.422));
        assert_eq!(maxmind.longitude, Some(-122.085));
        assert_eq!(maxmind.sources.location, Some("maxmind+dbip"));
    }

    #[test]
    fn unknown_dbip_country_does_not_override_known_maxmind_location() {
        let mut maxmind = GeoLocation {
            country_code: Some("US".into()), latitude: Some(37.751),
            longitude: Some(-97.822),
            sources: GeoSources { location: Some("maxmind"), ..GeoSources::default() },
            ..GeoLocation::default()
        };
        maxmind.fill_missing(GeoLocation {
            country_code: None,
            city: Some("Unknown".into()), latitude: Some(10.0), longitude: Some(20.0),
            sources: GeoSources { location: Some("dbip"), ..GeoSources::default() },
            ..GeoLocation::default()
        });
        assert!(maxmind.city.is_none());
        assert_eq!(maxmind.latitude, Some(37.751));
        assert_eq!(maxmind.sources.location, Some("maxmind"));
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
