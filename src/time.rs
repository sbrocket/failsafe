use anyhow::{format_err, Context, Result};
use chrono::{DateTime, TimeZone};
use chrono_tz::Tz;
use dtparse::Parser;
use enum_iterator::IntoEnumIterator;
use lazy_static::lazy_static;
use std::{collections::HashMap, iter};

const TZ_HACK_BASE: i32 = 100;

// This macro defines an enum that lists all the chrono_tz::Tz timezones that are used below. To
// work around dtparse::Parser requiring tzinfos to be defined in terms of a fixed offset (used with
// chrono::FixedOffset::east), we map each enumerator to a fake offset that is unlikely to ever be
// specified explicitly in a datetime string and then map the FixedOffset that Parser returns back
// to the actual chrono_tz::Tz we want.
macro_rules! used_timezones {
    ($($tz_name:ident),+ $(,)?) => {
        #[derive(IntoEnumIterator, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Copy)]
        enum TzHack {
            $($tz_name),+
        }

        impl TzHack {
            pub fn fake_offset(&self) -> i32 {
                TZ_HACK_BASE + *self as i32
            }

            fn timezone(&self) -> Tz {
                match self {
                    $(Self::$tz_name => Tz::$tz_name),+
                }
            }

            pub fn fake_offset_to_timezone(offset: i32) -> Result<Tz> {
                Self::into_enum_iter()
                    .find(|e| offset == TZ_HACK_BASE + *e as i32)
                    .map(|e| e.timezone())
                    .ok_or_else(|| format_err!("Got unexpected offset: {}", offset))
            }
        }
    }
}

used_timezones! {
    EST5EDT,
    CST6CDT,
    MST7MDT,
    PST8PDT,
}

// TODO: Expand list of supported timezones.
lazy_static! {
    static ref TZINFO: HashMap<String, i32> = {
        vec![
            (["ET", "EST", "EDT"], TzHack::EST5EDT),
            (["CT", "CST", "CDT"], TzHack::CST6CDT),
            (["MT", "MST", "MDT"], TzHack::MST7MDT),
            (["PT", "PST", "PDT"], TzHack::PST8PDT),
        ]
        .into_iter()
        .map(|(tz_abbrevs, tz)| {
            tz_abbrevs
                .iter()
                .map(|s| s.to_string())
                .zip(iter::repeat(tz.fake_offset()))
                .collect::<Vec<_>>()
        })
        .flatten()
        .collect()
    };
}

// TODO: This is very basic and can be improved but it does the basics.
// TODO: Would be neat to support relative dates, e.g. "8PM PT Friday"
pub fn parse_datetime(input: impl AsRef<str>) -> Result<DateTime<Tz>> {
    let input = input.as_ref();
    let (naive, tz_offset, _) = Parser::default().parse(
        input,
        Some(false),
        Some(false),
        false,
        false,
        None,
        false,
        &TZINFO,
    )?;

    // Use the parsed timezone or assume PDT timezone.
    match tz_offset {
        Some(tz_offset) => TzHack::fake_offset_to_timezone(tz_offset.local_minus_utc())
            .context("Fixed offset in datetime string?")?,
        None => Tz::PST8PDT,
    }
    .from_local_datetime(&naive)
    .single()
    .ok_or(format_err!("Ambiguous local time"))
}

/// Intended to be used with the #[serde(with = "module")] annotation on DateTime<Tz> fields
pub mod serialize_datetime_tz {
    use super::*;
    use chrono::Utc;
    use serde::{
        de::{Error, Unexpected},
        Deserialize, Deserializer, Serialize, Serializer,
    };
    use std::str::FromStr;

    #[derive(Serialize, Deserialize)]
    struct UtcDatetimeAndTimezone<'a>(DateTime<Utc>, &'a str);

    pub fn serialize<S>(dt: &DateTime<Tz>, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        Serialize::serialize(
            &UtcDatetimeAndTimezone(dt.with_timezone(&Utc), dt.timezone().name()),
            s,
        )
    }

    pub fn deserialize<'de, D>(d: D) -> Result<DateTime<Tz>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value: UtcDatetimeAndTimezone = Deserialize::deserialize(d)?;
        let tz = Tz::from_str(value.1)
            .map_err(|s| D::Error::invalid_value(Unexpected::Str(&s), &"a chrono_tz::Tz name"))?;
        Ok(value.0.with_timezone(&tz))
    }
}
