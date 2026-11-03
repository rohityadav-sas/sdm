use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One request. `kind` is "ping", "add" or "show".
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
pub struct Request {
    #[serde(default)]
    pub token: String,
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub url: String,
    /// Extra HTTP headers for the download (Cookie, Referer, User-Agent, ...).
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub filename: Option<String>,
    /// "extension", "cli", ...
    #[serde(default)]
    pub source: Option<String>,
}

impl Request {
    pub fn new(kind: &str) -> Self {
        Self { kind: kind.into(), ..Default::default() }
    }
    pub fn add(url: &str) -> Self {
        Self { kind: "add".into(), url: url.into(), ..Default::default() }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
pub struct Reply {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Only set by the native host: is the SDM app itself running?
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub running: Option<bool>,
}

impl Reply {
    pub fn ok() -> Self {
        Self { ok: true, ..Default::default() }
    }
    pub fn err(msg: impl Into<String>) -> Self {
        Self { ok: false, error: Some(msg.into()), ..Default::default() }
    }
}
