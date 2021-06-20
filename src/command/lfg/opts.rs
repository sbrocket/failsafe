use crate::command::OptionType;

define_command_option!(
    id: EventId,
    name: "event_id",
    description: "Event ID",
    required: true,
    option_type: OptionType::String(&[]),
);
