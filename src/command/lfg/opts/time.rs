use crate::{command::OptionType, util::*};
use chrono::{
    format::{self, StrftimeItems},
    DateTime, Datelike, Duration, TimeZone, Utc,
};
use chrono_tz::Tz;
use lazy_static::lazy_static;
use serde_json::Value;
use std::{
    cmp::Ordering,
    collections::HashMap,
    convert::{TryFrom, TryInto},
};
use thiserror::Error;
use tracing::warn;

define_command_option_group!(
    id: Datetime,
    options: [Date, TimeHour, TimeMinute, TimeAmPm, Timezone],
);

define_command_option!(
    id: Date,
    name: "date",
    description: "Event date as \"mm/dd\" (e.g. \"4/20\")",
    // TODO: Support relative dates
    //description: "Event date, either \"mm/dd\" (e.g. \"4/20\") or a day name (e.g. \"Friday\" for the next Friday)",
    required: true,
    option_type: OptionType::String(&[]),
);

define_command_option!(
    id: TimeHour,
    name: "hour",
    description: "Hour",
    required: true,
    option_type: OptionType::Integer(&[
        ("1", 1),
        ("2", 2),
        ("3", 3),
        ("4", 4),
        ("5", 5),
        ("6", 6),
        ("7", 7),
        ("8", 8),
        ("9", 9),
        ("10", 10),
        ("11", 11),
        ("12", 12)
    ]),
);

define_command_option!(
    id: TimeMinute,
    name: "minute",
    description: "Minute",
    required: true,
    option_type: OptionType::Integer(&[
        (":00", 0),
        (":15", 15),
        (":30", 30),
        (":45", 45),
    ]),
);

define_command_option!(
    id: TimeAmPm,
    name: "ampm",
    description: "AM/PM",
    required: true,
    option_type: OptionType::String(&[("AM", "AM"), ("PM", "PM")]),
);

define_command_option!(
    id: Timezone,
    name: "timezone",
    description: "Time Zone",
    required: true,
    option_type: OptionType::String(&[("ET", "ET"), ("CT", "CT"), ("MT", "MT"), ("PT", "PT")]),
);

// TODO: Expand list of supported timezones.
lazy_static! {
    static ref TIMEZONE_MAP: HashMap<&'static str, Tz> = {
        vec![
            ("ET", Tz::EST5EDT),
            ("CT", Tz::CST6CDT),
            ("MT", Tz::MST7MDT),
            ("PT", Tz::PST8PDT),
        ]
        .into_iter()
        .collect()
    };
}

#[derive(Error, Debug)]
pub enum DatetimeParseError {
    #[error("Unable to parse date '{0}': {1}")]
    InvalidDateFormat(String, #[source] format::ParseError),
    #[error("Date '{0}' is out of range: {1}")]
    DateOutOfRange(String, #[source] format::ParseError),
    #[error("{0} today is in the past")]
    TimeHasPassed(String),
    #[error("{0} is too far in the future")]
    TooFarAway(String),
    #[error("{0} is a recent past date")]
    MaybeRecentPast(String),
    #[error(transparent)]
    OptionError(#[from] OptionError),
    #[error("Missing required option '{0}'")]
    MissingRequiredOption(&'static str),
    #[error("Unexpected value type for option '{0}': {1:?}")]
    UnexpectedValueType(&'static str, Value),
    #[error("Unexpected value for option '{0}': {1:?}")]
    UnexpectedValue(&'static str, Value),
    #[error("Parsed rejected '{0}' value '{1}' unexpectedly: {2}")]
    ParsedRejectedValue(&'static str, String, #[source] format::ParseError),
    #[error("Parsed missing '{0}' value that should have already been parsed")]
    ParsedMissingValue(&'static str),
    #[error("NaiveTime creation failed: {0}, Parsed state: {1:?}")]
    NaiveTimeCreationFailed(#[source] format::ParseError, format::Parsed),
    #[error("Final DateTime<Tz> creation failed: {0}, Parsed state: {1:?}")]
    DatetimeCreationFailed(#[source] format::ParseError, format::Parsed),
}

impl DatetimeParseError {
    /// If the error was the result of user input, this returns a user-facing description of the
    /// error. Otherwise None.
    pub fn user_error(&self) -> Option<String> {
        match self {
            DatetimeParseError::InvalidDateFormat(date, _) => Some(format!(
                "'{}' isn't a valid date format; I need the month and day in that order (e.g. '2/20')",
                date
            )),
            DatetimeParseError::DateOutOfRange(date, _) => Some(format!(
                "'{}' is out-of-range and not a valid date.",
                date
            )),
            DatetimeParseError::TimeHasPassed(time) => Some(format!(
                "I can't do that, {} today is in the past... *I'm an AI, not a time-traveling Vex*",
                time
            )),
            DatetimeParseError::TooFarAway(date) => Some(format!(
                "I can't do that, {} is too far in the future.",
                date
            )),
            DatetimeParseError::MaybeRecentPast(date) => {
                Some(format!("I can't do that, {} is in the past.", date))
            }
            // All other error types are bugs/internal errors.
            _ => None,
        }
    }
}

pub fn parse_datetime_options<O: OptionsExt>(
    options: O,
) -> Result<DateTime<Tz>, DatetimeParseError> {
    use DatetimeParseError::*;

    let date = match options.get_value("date")? {
        Some(Value::String(v)) => Ok(v),
        Some(v) => Err(UnexpectedValueType("date", v.clone())),
        None => Err(MissingRequiredOption("date")),
    }?;
    let hour = match options.get_value("hour")? {
        Some(Value::Number(num)) => num
            .as_i64()
            .ok_or_else(|| UnexpectedValue("hour", Value::Number(num.clone()))),
        Some(v) => Err(UnexpectedValueType("hour", v.clone())),
        None => Err(MissingRequiredOption("hour")),
    }?;
    let minute = match options.get_value("minute")? {
        Some(Value::Number(num)) => num
            .as_i64()
            .ok_or_else(|| UnexpectedValue("minute", Value::Number(num.clone()))),
        Some(v) => Err(UnexpectedValueType("minute", v.clone())),
        None => Err(MissingRequiredOption("minute")),
    }?;
    let pm = match options.get_value("ampm")? {
        Some(Value::String(v)) => match v.as_str() {
            "AM" => Ok(false),
            "PM" => Ok(true),
            _ => Err(UnexpectedValue("ampm", Value::String(v.clone()))),
        },
        Some(v) => Err(UnexpectedValueType("ampm", v.clone())),
        None => Err(MissingRequiredOption("ampm")),
    }?;
    let timezone_str = match options.get_value("timezone")? {
        Some(Value::String(v)) => Ok(v),
        Some(v) => Err(UnexpectedValueType("timezone", v.clone())),
        None => Err(MissingRequiredOption("timezone")),
    }?;
    let timezone = *TIMEZONE_MAP
        .get(timezone_str.as_str())
        .ok_or_else(|| UnexpectedValue("timezone", Value::String(timezone_str.clone())))?;

    // TODO: Split function here and add unit tests over the latter part, especially around year
    // logic and leap (or other sometimes-valid) days. Like, what happens with "2/29" if it's
    // currently "3/1" and next year is or isn't a leap year, or if current year is or isn't.
    DatetimeComponents {
        now: Utc::now(),
        date,
        hour,
        minute,
        pm,
        timezone_str,
        timezone,
    }
    .try_into()
}

struct DatetimeComponents<'a> {
    now: DateTime<Utc>,
    date: &'a str,
    hour: i64,
    minute: i64,
    pm: bool,
    timezone_str: &'a str,
    timezone: Tz,
}

impl TryFrom<DatetimeComponents<'_>> for DateTime<Tz> {
    type Error = DatetimeParseError;

    fn try_from(value: DatetimeComponents) -> Result<Self, Self::Error> {
        use DatetimeParseError::*;

        let mut parsed = format::Parsed::new();
        format::parse(&mut parsed, value.date, StrftimeItems::new("%m/%d"))
            .map_err(|err| InvalidDateFormat(value.date.to_owned(), err))?;
        parsed
            .set_hour12(value.hour)
            .map_err(|err| ParsedRejectedValue("hour", value.hour.to_string(), err))?;
        parsed
            .set_minute(value.minute)
            .map_err(|err| ParsedRejectedValue("minute", value.minute.to_string(), err))?;
        parsed
            .set_ampm(value.pm)
            .map_err(|err| ParsedRejectedValue("ampm", value.pm.to_string(), err))?;

        // Figure out the year to use based on relation to the current date and on the fact that dates
        // shouldn't be in the past.
        //
        // If the calendar date is after the current date, use the current year. If it is before the
        // current date, use the next year. If it is the current date, we use the current year but
        // require that the time be in the future.
        //
        // For example, if the current date is "12/12/2021", an input of "12/15" will use 2021 as the
        // year and an input of "1/10" will use 2022. This also means that "12/11" will use 2022, even
        // though the user may be mistakenly using the wrong date and intended the current year. This
        // will be caught later, e.g. by checking that the date is no more than X months away.
        let now = value.now.with_timezone(&value.timezone);
        let month = parsed.month.ok_or_else(|| ParsedMissingValue("month"))?;
        let day = parsed.day.ok_or_else(|| ParsedMissingValue("day"))?;
        let next_year = match month.cmp(&now.month()) {
            Ordering::Less => true,
            Ordering::Equal => match day.cmp(&now.day()) {
                Ordering::Less => true,
                Ordering::Equal => {
                    let time = parsed
                        .to_naive_time()
                        .map_err(|err| NaiveTimeCreationFailed(err, parsed.clone()))?;
                    let now_time = now.time();
                    if time >= now_time {
                        false
                    } else {
                        let mut time_str = time.format("%-I:%M %p ").to_string();
                        time_str.push_str(value.timezone_str);
                        return Err(TimeHasPassed(time_str));
                    }
                }
                Ordering::Greater => false,
            },
            Ordering::Greater => false,
        };
        let year = now.year() + if next_year { 1 } else { 0 };
        let datetime =
            datetime_with_timezone_for_year(parsed.clone(), value.timezone, year.into())?;

        const FUTURE_DATE_LIMIT_WEEKS: i64 = 26;
        const RECENT_PAST_DATE_DAYS: i64 = 30;

        // Check whether the resulting date is unreasonably far away (arbitrarily chosen as ~6 months or
        // 26 weeks). If so, return an error. The error is either:
        //   - that the date is too far in the future or,
        //   - if using the current year instead makes the date less than a ~month (30 days) ago,
        //   assume the user meant that and they have the wrong date.
        let date_str = |dt: DateTime<Tz>| dt.format("%-m/%-d/%-Y").to_string();
        if datetime - now >= Duration::weeks(FUTURE_DATE_LIMIT_WEEKS) {
            if next_year {
                let alternate_datetime =
                    datetime_with_timezone_for_year(parsed, value.timezone, now.year().into());
                match alternate_datetime {
                    Ok(alt) => {
                        if now - alt <= Duration::days(RECENT_PAST_DATE_DAYS) {
                            return Err(MaybeRecentPast(date_str(alt)));
                        }
                    }
                    Err(err) => warn!(
                        "Error checking if alternate date is in recent past: {:?}",
                        err
                    ),
                }
            }

            return Err(TooFarAway(date_str(datetime)));
        }
        Ok(datetime)
    }
}

fn datetime_with_timezone_for_year<Tz: TimeZone>(
    mut parsed: format::Parsed,
    timezone: Tz,
    year: i64,
) -> Result<DateTime<Tz>, DatetimeParseError> {
    use DatetimeParseError::*;

    parsed
        .set_year(year.into())
        .map_err(|err| ParsedRejectedValue("year", year.to_string(), err))?;

    match parsed.to_datetime_with_timezone(&timezone) {
        Ok(dt) => Ok(dt),
        Err(err) => Err(match err.kind() {
            format::ParseErrorKind::OutOfRange => {
                let month = parsed.month.ok_or_else(|| ParsedMissingValue("month"))?;
                let day = parsed.day.ok_or_else(|| ParsedMissingValue("day"))?;
                let year = parsed.year.ok_or_else(|| ParsedMissingValue("year"))?;
                DateOutOfRange(format!("{}/{}/{}", month, day, year), err)
            }
            _ => DatetimeCreationFailed(err, parsed),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use test_env_log::test;
    use DatetimeParseError::*;

    macro_rules! test_parse {
        ($(
            $test_name:ident => {
                now: $now:literal,
                date: $date:literal,
                hour: $hour:literal,
                minute: $minute:literal,
                pm: $pm:literal,
                timezone: $timezone_str:literal,
                pattern: $($pat:tt)*
            }
        ),+ $(,)? ) => {
            $(
                #[test]
                fn $test_name() {
                    let now = DateTime::parse_from_rfc3339($now).expect("Bad now RFC3339 date").with_timezone(&Utc);
                    let timezone = *TIMEZONE_MAP.get($timezone_str).expect("Unknown timezone");

                    let result = <DateTime<Tz>>::try_from(DatetimeComponents {
                        now,
                        date: $date,
                        hour: $hour,
                        minute: $minute,
                        pm: $pm,
                        timezone_str: $timezone_str,
                        timezone,
                    });
                    assert_matches!(result, $($pat)*);
                }
            )+
        };
    }

    macro_rules! test_parse_ok {
        ($(
            $test_name:ident => {
                now: $now:literal,
                date: $date:literal,
                hour: $hour:literal,
                minute: $minute:literal,
                pm: $pm:literal,
                timezone: $timezone_str:literal,
                expected: $expected:literal,
            }
        ),+ $(,)? ) => {
            $(
                test_parse! {
                    $test_name => {
                        now: $now,
                        date: $date,
                        hour: $hour,
                        minute: $minute,
                        pm: $pm,
                        timezone: $timezone_str,
                        pattern: Ok(dt) => {
                            let expected = DateTime::parse_from_rfc3339($expected).expect("Bad expected RFC3339 date");
                            assert_eq!(dt, expected);
                        }
                    }
                }
            )+
        };
    }

    test_parse_ok! {
        same_day => {
            now: "2021-04-20T14:00:00-04:00",
            date: "4/20",
            hour: 2,
            minute: 15,
            pm: true,
            timezone: "ET", // EDT (UTC-4) on 4/20
            expected: "2021-04-20T14:15:00-04:00",
        },
        same_month => {
            now: "2021-04-20T00:00:00Z",
            date: "4/22",
            hour: 2,
            minute: 15,
            pm: true,
            timezone: "ET", // EDT (UTC-4) on 4/22
            expected: "2021-04-22T14:15:00-04:00",
        },
        future_month => {
            now: "2021-04-20T00:00:00Z",
            date: "6/22",
            hour: 10,
            minute: 45,
            pm: false,
            timezone: "MT", // MDT (UTC-6) on 6/22
            expected: "2021-06-22T10:45:00-06:00",
        },
        next_year => {
            now: "2021-12-01T00:00:00Z",
            date: "1/5",
            hour: 8,
            minute: 30,
            pm: true,
            timezone: "CT", // CST (UTC-6) on 1/5
            expected: "2022-01-05T20:30:00-06:00",
        },
        padded_date => {
            now: "2021-01-05T14:00:00-04:00",
            date: "01/08",
            hour: 2,
            minute: 0,
            pm: true,
            timezone: "PT", // PST (UTC-8) on 1/8
            expected: "2021-01-08T14:00:00-08:00",
        },
        leap_day => {
            now: "2020-02-01T00:00:00Z",
            date: "2/29",
            hour: 8,
            minute: 30,
            pm: true,
            timezone: "CT", // CST (UTC-6) on 2/29
            expected: "2020-02-29T20:30:00-06:00",
        },
    }

    test_parse! {
         earlier_today => {
             now: "2021-04-20T15:00:00-04:00",
             date: "4/20",
             hour: 2,
             minute: 30,
             pm: true,
             timezone: "ET", // EDT (UTC-4) on 4/20
             pattern: Err(TimeHasPassed(time)) if time == "2:30 PM ET"
         },
         invalid_date1 => {
             now: "2021-04-20T12:00:00-04:00",
             date: "4/",
             hour: 2,
             minute: 30,
             pm: true,
             timezone: "ET",
             pattern: Err(InvalidDateFormat(date, _)) if date == "4/"
         },
         invalid_date2 => {
             now: "2021-04-20T12:00:00-04:00",
             date: "4-20",
             hour: 2,
             minute: 30,
             pm: true,
             timezone: "ET",
             pattern: Err(InvalidDateFormat(date, _)) if date == "4-20"
         },
        month_out_of_range => {
            now: "2021-02-01T00:00:00Z",
            date: "13/1",
            hour: 8,
            minute: 30,
            pm: true,
            timezone: "CT",
            pattern: Err(DateOutOfRange(date, _)) if date == "13/1/2021"
        },
        day_out_of_range1 => {
            now: "2021-02-01T00:00:00Z",
            date: "1/32",
            hour: 8,
            minute: 30,
            pm: true,
            timezone: "CT",
            pattern: Err(DateOutOfRange(date, _)) if date == "1/32/2021"
        },
        day_out_of_range2 => {
            now: "2021-02-01T00:00:00Z",
            date: "4/31",
            hour: 8,
            minute: 30,
            pm: true,
            timezone: "CT",
            pattern: Err(DateOutOfRange(date, _)) if date == "4/31/2021"
        },
        not_leap_year => {
            now: "2021-02-01T00:00:00Z",
            date: "2/29",
            hour: 8,
            minute: 30,
            pm: true,
            timezone: "CT",
            pattern: Err(DateOutOfRange(date, _)) if date == "2/29/2021"
        },
        too_far_away1 => {
            now: "2021-02-01T00:00:00Z",
            date: "10/1",
            hour: 1,
            minute: 0,
            pm: true,
            timezone: "CT",
            pattern: Err(TooFarAway(date)) if date == "10/1/2021"
        },
        too_far_away2 => {
            now: "2021-10-01T00:00:00Z",
            date: "6/1",
            hour: 1,
            minute: 0,
            pm: true,
            timezone: "CT",
            pattern: Err(TooFarAway(date)) if date == "6/1/2022"
        },
        recent_past1 => {
            now: "2021-02-10T10:00:00-06:00",
            date: "2/9",
            hour: 1,
            minute: 0,
            pm: true,
            timezone: "CT", // CST (UTC-6) on 2/10
            pattern: Err(MaybeRecentPast(date)) if date == "2/9/2021"
        },
        recent_past2 => {
            now: "2021-02-10T10:00:00-06:00",
            date: "1/30",
            hour: 1,
            minute: 0,
            pm: true,
            timezone: "CT", // CST (UTC-6) on 2/10
            pattern: Err(MaybeRecentPast(date)) if date == "1/30/2021"
        },
    }
}
