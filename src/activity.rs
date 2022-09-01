use enum_iterator::IntoEnumIterator;
use serde::{Deserialize, Serialize};

macro_rules! with_activity_types {
    ($macro:ident) => {
        $macro! {
            Raid: ("Raid", "raid"),
            Dungeon: ("Dungeon", "dungeon"),
            Crucible: ("Crucible", "pvp"),
            Gambit: ("Gambit", "gambit", Single),
            PvE: ("PvE", "pve"),
            Seasonal: ("Seasonal", "seasonal"),
            Custom: ("Custom", "custom", Single),
        }
    };
}

macro_rules! define_activity_types {
    ($($enum_name:ident: ($name:literal, $cmd:literal $(, Single)?)),+ $(,)?) => {
        /// Destiny 2 activity types.
        #[derive(IntoEnumIterator, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Copy, Serialize, Deserialize)]
        pub enum ActivityType {
            $($enum_name),+
        }

        impl ActivityType {
            #[allow(dead_code)]
            pub fn name(&self) -> &'static str {
                match self {
                    $(Self::$enum_name => $name),+
                }
            }

            #[allow(dead_code)]
            pub fn command_name(&self) -> &'static str {
                match self {
                    $(Self::$enum_name => $cmd),+
                }
            }
        }
    }
}

with_activity_types! { define_activity_types }
static_assertions::const_assert!(ActivityType::VARIANT_COUNT <= 25);

macro_rules! define_activities {
    ($($enum_name:ident: ($name:literal, $prefix:literal, $activity_type:ident, $group_size:literal)),+ $(,)?) => {
        /// All supported Destiny 2 activities.
        #[derive(IntoEnumIterator, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Copy, Serialize, Deserialize)]
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

            pub fn activities_with_type(ty: ActivityType) -> impl Iterator<Item = Activity> {
                Self::into_enum_iter().filter(move |a| a.activity_type() == ty)
            }

            pub fn activity_with_id_prefix(prefix: impl AsRef<str>) -> Option<Activity> {
                let prefix = prefix.as_ref();
                Self::into_enum_iter().find(|a| a.id_prefix() == prefix)
            }

            pub fn default_group_size(&self) -> u8 {
                match self {
                    $(Self::$enum_name => $group_size),+
                }
            }
        }

        impl std::fmt::Display for Activity {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.name())
            }
        }
    };
}

define_activities! {
    KingsFall: ("King's Fall", "kf", Raid, 6),
    VowOfTheDisciple: ("Vow of the Disciple", "votd", Raid, 6),
    VaultOfGlass: ("Vault of Glass", "vog", Raid, 6),
    DeepStoneCrypt: ("Deep Stone Crypt", "dsc", Raid, 6),
    GardenOfSalvation: ("Garden of Salvation", "gos", Raid, 6),
    LastWish: ("Last Wish", "lw", Raid, 6),
    Duality: ("Duality", "dual", Dungeon, 3),
    GraspOfAvarice: ("GraspOfAvarice", "goa", Dungeon, 3),
    Prophecy: ("Prophecy", "proph", Dungeon, 3),
    PitOfHeresy: ("Pit of Heresy", "pit", Dungeon, 3),
    ShatteredThrone: ("Shattered Throne", "throne", Dungeon, 3),
    IronBanner: ("Iron Banner", "ib", Crucible, 6),
    TrialsOfOsiris: ("Trials of Osiris", "trials", Crucible, 4),
    Quickplay: ("Quickplay", "quick", Crucible, 4),
    Competitive: ("Competitive", "comp", Crucible, 4),
    PrivateMatch: ("Private Match", "priv", Crucible, 6),
    OtherPvP: ("Other PvP", "pvp", Crucible, 6),
    Gambit: ("Gambit", "gambit", Gambit, 4),
    Grandmaster: ("Grandmaster Nightfall", "gm", PvE, 3),
    Nightfall: ("Nightfall", "nf", PvE, 3),
    Wellspring: ("Wellspring", "well", PvE, 6),
    Harbinger: ("Harbinger", "harb", PvE, 3),
    Presage: ("Presage", "pres", PvE, 3),
    Story: ("Story Missions", "story", PvE, 3),
    OtherPvE: ("Other PvE", "pve", PvE, 3),
    Override: ("Override", "override", Seasonal, 6),
    Battleground: ("Battleground", "battle", Seasonal, 3),
    WrathbornHunt: ("Wrathborn Hunt", "hunt", Seasonal, 3),
    Custom: ("Custom", "cust", Custom, 6),
}

#[cfg(test)]
mod tests {
    use super::*;
    use itertools::Itertools;
    use test_env_log::test;

    #[test]
    fn no_conflicting_activity_types() {
        assert_eq!(
            ActivityType::VARIANT_COUNT,
            ActivityType::into_enum_iter()
                .map(|a| a.command_name())
                .unique()
                .count()
        );
    }

    #[test]
    fn no_conflicting_activity_prefixes() {
        assert_eq!(
            Activity::VARIANT_COUNT,
            Activity::into_enum_iter()
                .map(|a| a.id_prefix())
                .unique()
                .count()
        );
    }

    #[test]
    fn activity_prefixes_alphabetic() {
        Activity::into_enum_iter().for_each(|a| {
            assert!(a.id_prefix().chars().all(|c| c.is_ascii_alphabetic()));
        })
    }

    #[test]
    fn max_activities_per_type() {
        ActivityType::into_enum_iter().for_each(|ty| {
            assert!(Activity::activities_with_type(ty).count() <= 25);
        })
    }

    // The annotation for which ActivityTypes have a single associated Activity is manually applied;
    // check that it is correct.
    #[test]
    fn activity_types_single_annotation_is_correct() {
        macro_rules! check_single_annotation {
            ($($enum_name:ident: $params:tt),+ $(,)?) => {
                $(
                    check_single_annotation!{ @each $enum_name: $params }
                )+
            };
            (@each $enum_name:ident: ($name:literal, $cmd:literal, Single)) => {
                assert_eq!(
                    Activity::activities_with_type(ActivityType::$enum_name).count(),
                    1
                )
            };
            (@each $enum_name:ident: ($name:literal, $cmd:literal)) => { };
        }

        with_activity_types! { check_single_annotation }
    }
}
