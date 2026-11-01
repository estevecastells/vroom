//! Cloudflare R2 over its S3 API: signed requests from `rusty-s3`, sent with a
//! small blocking HTTP client. No async runtime and one part in memory at a time.

pub mod r2;
pub mod sync;

pub use r2::{Object, R2};
