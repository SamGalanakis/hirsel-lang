//! The standard-library suites of this crate: ordinary std source compiled,
//! checked and executed through the driver, linked as their own binary so a
//! compiler edit relinks three smaller harnesses in parallel.
#[path = "../support/source_std.rs"]
mod source_std;
mod source_std_cache;
mod stdlib_graphemes;
mod stdlib_http;
mod stdlib_http_wire;
mod stdlib_integer_policy;
mod stdlib_iter;
mod stdlib_json;
mod stdlib_net;
mod stdlib_nfc;
mod stdlib_origin_work;
mod stdlib_pathtime;
mod stdlib_patterns;
mod stdlib_testhelpers;
mod stdlib_text_wire;
mod stdlib_uri;
mod stdlib_wave1;
mod stdlib_wave2_num;
