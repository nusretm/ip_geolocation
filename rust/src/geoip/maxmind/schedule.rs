use chrono::{Datelike, Utc, Weekday};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DatabaseKind {
    City,
    Country,
    Asn,
}

impl DatabaseKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::City => "city",
            Self::Country => "country",
            Self::Asn => "asn",
        }
    }

    pub fn expected_type(self) -> &'static str {
        match self {
            Self::City => "City",
            Self::Country => "Country",
            Self::Asn => "ASN",
        }
    }

    pub fn supports_edition(self, edition: &str) -> bool {
        matches!(
            (self, edition),
            (Self::City, "GeoLite2-City" | "GeoIP2-City")
                | (Self::Country, "GeoLite2-Country" | "GeoIP2-Country")
                | (Self::Asn, "GeoLite2-ASN")
        )
    }

    pub fn publishes_on(self, edition: &str, weekday: Weekday) -> bool {
        if edition.starts_with("GeoIP2-") {
            return !matches!(weekday, Weekday::Sat | Weekday::Sun);
        }
        match self {
            Self::Asn => !matches!(weekday, Weekday::Sat | Weekday::Sun),
            Self::City | Self::Country => matches!(weekday, Weekday::Tue | Weekday::Fri),
        }
    }

    pub fn check_interval_seconds(self, edition: &str, release_hours: u64, other_hours: u64) -> u64 {
        let hours = if self.publishes_on(edition, Utc::now().weekday()) {
            release_hours
        } else {
            other_hours
        };
        hours.saturating_mul(3600)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geolite_city_and_country_publish_tuesday_friday() {
        assert!(DatabaseKind::City.publishes_on("GeoLite2-City", Weekday::Tue));
        assert!(DatabaseKind::Country.publishes_on("GeoLite2-Country", Weekday::Fri));
        assert!(!DatabaseKind::City.publishes_on("GeoLite2-City", Weekday::Mon));
        assert!(!DatabaseKind::Country.publishes_on("GeoLite2-Country", Weekday::Sat));
    }

    #[test]
    fn asn_is_weekdays_and_commercial_city_is_weekdays() {
        assert!(DatabaseKind::Asn.publishes_on("GeoLite2-ASN", Weekday::Mon));
        assert!(!DatabaseKind::Asn.publishes_on("GeoLite2-ASN", Weekday::Sun));
        assert!(DatabaseKind::City.publishes_on("GeoIP2-City", Weekday::Thu));
        assert!(!DatabaseKind::City.supports_edition("GeoLite2-ASN"));
    }
}
