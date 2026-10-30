//! csvfab's engine: reading, editing, filtering, sorting and writing CSV files
//! of any size. No GUI here — the app crate draws what this computes.

pub mod anonymize;
pub mod base;
pub mod cifind;
pub mod colcache;
pub mod combine;
pub mod compare;
pub mod convert;
pub mod diagnose;
pub mod dupes;
pub mod edit;
pub mod enc;
pub mod fill_example;
pub mod formula;
pub mod group;
pub mod insights;
pub mod journal;
pub mod kinds;
pub mod look;
pub mod mapguard;
pub mod noise;
pub mod pivot;
pub mod radix;
pub mod re;
pub mod review;
pub mod save;
pub mod scan;
pub mod schema;
pub mod sparkline;
pub mod split;
pub mod stats;
pub mod table;
pub mod tail;
pub mod text;
pub mod view;

pub use base::{Base, Expect, OpenOptions, Stamp};
pub use enc::Enc;
pub use table::{RowId, State, Table};
