//! Windows SCM-specific integration only. The HTTP and provider core stays
//! cross-platform and can still run as a normal console process.
pub mod management;
pub mod runtime;

pub const SERVICE_NAME: &str = "RemVibeGeoLocation";
pub const DISPLAY_NAME: &str = "RemVibe GeoLocation Service";
