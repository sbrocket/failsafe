macro_rules! define_command_option {
    (
        id: $id:ident,
        name: $name:literal,
        description: $descr:literal,
        required: $required:literal,
        option_type: $option_type:expr,
    ) => {
        #[allow(non_snake_case)]
        pub mod $id {
            use super::*;

            lazy_static::lazy_static! {
                pub static ref OPTION: $crate::command::CommandOption = $crate::command::CommandOption {
                    name: $name,
                    description: $descr,
                    required: $required,
                    option_type: $option_type,
                };
            }
        }
    };
}

macro_rules! define_leaf_command {
    ($id:ident, $name:literal, $descr:expr, $handler:ident, options: [$($($opt_path:ident)::+),* $(,)?],) => {
        #[allow(non_snake_case)]
        pub mod $id {
            #[allow(unused)]
            use super::*;

            lazy_static::lazy_static! {
                static ref OPTIONS: Vec<&'static $crate::command::CommandOption> = vec![
                    $(&*$($opt_path)::+ ::OPTION),*
                ];

                pub static ref LEAF: $crate::command::LeafCommand = $crate::command::LeafCommand {
                    options: &*OPTIONS,
                    handler: $handler,
                };

                pub static ref COMMAND: $crate::command::Command = $crate::command::Command {
                    name: $name,
                    description: $descr,
                    command_type: $crate::command::CommandType::Leaf(&*LEAF),
                };
            }
        }
    };
}

macro_rules! define_command_group {
    ($id:ident, $name:literal, $descr:literal, subcommands: [$($($sub_path:ident)::+),+ $(,)?]) => {
        #[allow(non_snake_case)]
        pub mod $id {
            #[allow(unused)]
            use super::*;

            lazy_static::lazy_static! {
                static ref COMMANDS: Vec<&'static $crate::command::Command> = vec![
                    $(&*$($sub_path)::+ ::COMMAND),*
                ];

                pub static ref COMMAND: $crate::command::Command = $crate::command::Command {
                    name: $name,
                    description: $descr,
                    command_type: $crate::command::CommandType::Group(&*COMMANDS),
                };
            }
        }
    };
}
