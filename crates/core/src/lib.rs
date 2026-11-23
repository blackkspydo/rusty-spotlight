//! Pure search engine: items, tiers, matching, learned preference, ranking.
//! No OS types and no I/O, so it builds and tests on any platform.

pub mod brew;
pub mod calc;
pub mod rank;
pub mod settings;
pub mod text;
pub mod time;
pub mod units;
pub mod usage;

pub use rank::{Item, Kind, Ranked, Ranker, Tier};
pub use usage::Usage;
