macro_rules! define_command {
    ($ty:ident, $name:literal, $descr:literal, Leaf) => {
        #[derive(Debug)]
        pub struct $ty;

        impl $ty {
            pub fn new() -> Self {
                $ty
            }
        }

        impl $crate::command::Command for $ty {
            fn name(&self) -> &'static str {
                $name
            }

            fn description(&self) -> &'static str {
                $descr
            }

            fn command_type(&self) -> $crate::command::CommandType {
                $crate::command::CommandType::Leaf(self as &dyn LeafCommand)
            }
        }
    };
    ($ty:ident, $name:literal, $descr:literal, Subcommands: [$($sub_ty:ident),+ $(,)?]) => {
        #[derive(Debug)]
        pub struct $ty {
            subcommands: $crate::command::SubcommandsMap,
        }

        impl $ty {
            pub fn new() -> Self {
                Self {
                    subcommands: vec![
                        $(Box::new($sub_ty::new()) as Box<dyn LeafCommand>),+
                    ]
                    .into_iter()
                    .map(|c| (c.name(), c))
                    .collect(),
                }
            }
        }

        impl $crate::command::Command for $ty {
            fn name(&self) -> &'static str {
                $name
            }

            fn description(&self) -> &'static str {
                $descr
            }

            fn command_type(&self) -> $crate::command::CommandType {
                $crate::command::CommandType::Subcommands(&self.subcommands)
            }
        }
    };
}
