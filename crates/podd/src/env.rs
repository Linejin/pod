//! Terminal environment hygiene: every item of [`pod_client::env`], under its
//! old path (card D-T1).
//!
//! The functions are pure map transforms and live in `pod-client`; the daemon
//! keeps the I/O around them (the user's registry environment in `winsec`,
//! the git config it reads for the worker guard).

pub use pod_client::env::*;
