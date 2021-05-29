use serde::{Deserialize, Serialize};

/// Destiny 2 activity types.
// TODO: Fill out other activity types.
#[derive(Serialize, Deserialize)]
pub enum ActivityType {
    Raid,
}

macro_rules! define_activities {
    ($($enum_name:ident => ($name:literal, $prefix:literal, $activity_type:ident)),+ $(,)?) => {
        /// All supported Destiny 2 activities.
        #[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Copy, Serialize, Deserialize)]
        pub enum Activity {
            $($enum_name),+
        }

        impl Activity {
            pub fn name(&self) -> &'static str {
                match self {
                    $(Self::$enum_name => $name),+
                }
            }

            pub fn id_prefix(&self) -> &'static str {
                match self {
                    $(Self::$enum_name => $prefix),+
                }
            }

            pub fn activity_type(&self) -> ActivityType {
                match self {
                    $(Self::$enum_name => ActivityType::$activity_type),+
                }
            }
        }
    };
}

// TODO: Fill out other activities.
define_activities! {
    VaultOfGlass => ("Vault of Glass", "vog", Raid),
    DeepStoneCrypt => ("Deep Stone Crypt", "dsc", Raid),
    GardenOfSalvation => ("Garden of Salvation", "gos", Raid),
    LastWish => ("Last Wish", "lw", Raid),
}
