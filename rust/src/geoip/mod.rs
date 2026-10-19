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

pub struct GeoService {
    providers: Vec<Arc<dyn GeoProviderAbs>>,
    maxmind_readers: Option<Arc<maxmind::MaxMindReaders>>,
}

impl GeoService {
    pub fn load(config: &GeoConfig) -> Result<Self> {
        let mut providers: Vec<Arc<dyn GeoProviderAbs>> = Vec::new();
        let mut maxmind_readers = None;
        if let Some(provider) = maxmind::MaxMindProvider::load(&config.maxmind)? {
            maxmind_readers = Some(provider.live_readers());
            providers.push(Arc::new(provider));
        }
        Ok(Self { providers, maxmind_readers })
    }

    pub fn maxmind_live_readers(&self) -> Option<Arc<maxmind::MaxMindReaders>> {
        self.maxmind_readers.clone()
    }

    pub fn has_providers(&self) -> bool {
        !self.providers.is_empty()
    }

    pub fn lookup(&self, ip: IpAddr) -> Result<Option<GeoLocation>> {
        for provider in &self.providers {
            if let Some(location) = provider.lookup(ip)? {
                return Ok(Some(location));
            }
        }
        Ok(None)
    }
}
