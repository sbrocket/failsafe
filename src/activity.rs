use enum_iterator::IntoEnumIterator;
use serde::{Deserialize, Serialize};

macro_rules! with_activity_types {
    ($macro:ident) => {
        $macro! {
            Raid: ("Raid", "raid"),
            Dungeon: ("Dungeon", "dungeon"),
            Crucible: ("Crucible", "pvp"),
            Gambit: ("Gambit", "gambit"),
            ExoticQuest: ("Exotic Quest", "exotic"),
            Seasonal: ("Seasonal", "seasonal"),
            Other: ("Other", "other"),
            Custom: ("Custom", "custom"),
        }
    };
}

macro_rules! define_activity_types {
    ($($enum_name:ident: ($name:literal, $cmd:literal)),+ $(,)?) => {
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
    ($($enum_name:ident: ($name:literal, $prefix:literal, $activity_type:ident)),+ $(,)?) => {
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
        }

        impl std::fmt::Display for Activity {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.name())
            }
        }
    };
}

// Other possible activities to add: EmpireHunt, LostSector, NightmareHunt, Quests, Strikes, Farming, Story, BlindWell,
define_activities! {
    VaultOfGlass: ("Vault of Glass", "vog", Raid),
    DeepStoneCrypt: ("Deep Stone Crypt", "dsc", Raid),
    GardenOfSalvation: ("Garden of Salvation", "gos", Raid),
    LastWish: ("Last Wish", "lw", Raid),
    Prophecy: ("Prophecy", "proph", Dungeon),
    PitOfHeresy: ("Pit of Heresy", "pit", Dungeon),
    ShatteredThrone: ("Shattered Throne", "throne", Dungeon),
    IronBanner: ("Iron Banner", "ib", Crucible),
    TrialsOfOsiris: ("Trials of Osiris", "trials", Crucible),
    Quickplay: ("Quickplay", "quick", Crucible),
    Competitive: ("Competitive", "comp", Crucible),
    Gambit: ("Gambit", "gambit", Gambit),
    Harbinger: ("Harbinger", "harb", ExoticQuest),
    Presage: ("Presage", "pres", ExoticQuest),
    Override: ("Override", "override", Seasonal),
    WrathbornHunt: ("Wrathborn Hunt", "hunt", Seasonal),
    Nightfall: ("Nightfall", "nf", Other),
    Custom: ("Custom", "cust", Custom),
}

#[cfg(test)]
mod tests {
    use super::*;
    use itertools::Itertools;

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
    fn max_activities_per_type() {
        ActivityType::into_enum_iter().for_each(|ty| {
            assert!(Activity::activities_with_type(ty).count() <= 25);
        })
    }
}
