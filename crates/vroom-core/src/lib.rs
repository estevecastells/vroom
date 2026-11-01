//! Vroom logic shared by every platform: the recording library on disk, the
//! config file, the cloud layout and the JSON the CLI speaks to the apps.
//! No network and no capture here; the apps record, `vroom-cloud` uploads.

pub mod config;
pub mod id;
pub mod layout;
pub mod library;
pub mod meta;
pub mod protocol;
pub mod share;

pub use config::{Config, R2Config};
pub use library::{Entry, Library, Status};
pub use meta::{Meta, Source};
