//! Plugin-side (guest) glue: the `guest` feature, wasm32 only.

use extism_pdk::Error;

use crate::contract::{HttpFetchRequest, HttpFetchResponse};

#[extism_pdk::host_fn]
extern "ExtismHost" {
    fn http_fetch(request: String) -> String;
    fn get_credential(source: String) -> String;
}

/// Sends a request through the host's allowlisted fetcher.
pub fn fetch(req: &HttpFetchRequest) -> Result<HttpFetchResponse, Error> {
    let raw = unsafe { http_fetch(serde_json::to_string(req)?)? };
    Ok(serde_json::from_str(&raw)?)
}

/// Returns this plugin's credentials as a JSON object string.
pub fn credentials() -> Result<String, Error> {
    unsafe { get_credential(String::new()) }
}
