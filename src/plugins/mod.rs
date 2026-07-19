//! Scraper/plugin host: the WASM runtime, the capability-injected scraper
//! contract, the plugin store + marketplace/index, browse response caching, and
//! the calendar source.

pub mod browse_cache;
pub mod calendar;
pub mod marketplace;
pub mod plugin_index;
pub mod plugin_store;
pub mod scraper;
pub mod wasm_host;
