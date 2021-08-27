use crate::{command::OptionType, util::*};
use chrono::{
    format::{self, StrftimeItems},
    DateTime, Datelike, Utc,
};
use chrono_tz::Tz;
use lazy_static::lazy_static;
use serde_json::Value;
use std::{cmp::Ordering, collections::HashMap};
use thiserror::Error;

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
    InvalidDate(String, #[source] format::ParseError),
    #[error("{0} today is in the past")]
    TimeHasPassed(String),
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
            DatetimeParseError::InvalidDate(date, _) => Some(format!(
                "'{}' isn't a valid date; I need the month and day in that order (e.g. '2/20')",
                date
            )),
            DatetimeParseError::TimeHasPassed(time) => Some(format!(
                "I can't do that, {} today is in the past... *I'm an AI, not a time-traveling Vex*",
                time
            )),
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
    let ampm = match options.get_value("ampm")? {
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
    let timezone = TIMEZONE_MAP
        .get(timezone_str.as_str())
        .ok_or_else(|| UnexpectedValue("timezone", Value::String(timezone_str.clone())))?;

    // TODO: Split function here and add unit tests over the latter part, especially around year
    // logic and leap (or other sometimes-valid) days. Like, what happens with "2/29" if it's
    // currently "3/1" and next year is or isn't a leap year, or if current year is or isn't.

    let mut parsed = format::Parsed::new();
    format::parse(&mut parsed, date, StrftimeItems::new("%m/%d"))
        .map_err(|err| InvalidDate(date.clone(), err))?;
    parsed
        .set_hour12(hour)
        .map_err(|err| ParsedRejectedValue("hour", hour.to_string(), err))?;
    parsed
        .set_minute(minute)
        .map_err(|err| ParsedRejectedValue("minute", minute.to_string(), err))?;
    parsed
        .set_ampm(ampm)
        .map_err(|err| ParsedRejectedValue("ampm", ampm.to_string(), err))?;

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
    let now = Utc::now().with_timezone(timezone);
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
                    let mut time = time.format("%-I:%M %p ").to_string();
                    time.push_str(timezone_str);
                    return Err(TimeHasPassed(time));
                }
            }
            Ordering::Greater => false,
        },
        Ordering::Greater => false,
    };
    let year = now.year() + if next_year { 1 } else { 0 };
    parsed
        .set_year(year.into())
        .map_err(|err| ParsedRejectedValue("year", year.to_string(), err))?;

    parsed
        .to_datetime_with_timezone(timezone)
        .map_err(|err| DatetimeCreationFailed(err, parsed))
}
