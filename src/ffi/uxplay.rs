//! Bindings an den UxPlay-Kern (`raop.h`, `stream.h`, `logger.h`, `dnssd.h`, `utils.h`).

#![allow(
    non_upper_case_globals,
    non_camel_case_types,
    non_snake_case,
    dead_code,
    clippy::all
)]

include!(concat!(env!("OUT_DIR"), "/uxplay.rs"));
