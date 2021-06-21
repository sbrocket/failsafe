use crate::command::OptionType;

define_command_option!(
    id: EventId,
    name: "event_id",
    description: "Event ID",
    required: true,
    option_type: OptionType::String(&[]),
);

define_command_option!(
    id: Datetime,
    name: "datetime",
    description: "Date & time for this event, in \"h:m am/pm tz mm/dd\" format (e.g. \"8:00 PM CT 4/20\")",
    required: true,
    option_type: OptionType::String(&[]),
);
