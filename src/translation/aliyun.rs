use std::{collections::BTreeMap, time::Duration};

use anyhow::{Context, Result, bail, ensure};
use chrono::{SecondsFormat, Utc};
use hmac::{Hmac, Mac};
use reqwest::{blocking::Client, redirect::Policy};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::settings::Credentials;

const HOST: &str = "mt.aliyuncs.com";
pub const CONSOLE: &str = "https://mt.console.aliyun.com/";
pub const DEFAULT_LANGUAGE: &str = "zh";

pub struct Aliyun {
    client: Client,
    credentials: Credentials,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Response {
    #[serde(default)]
    code: serde_json::Value,
    #[serde(default)]
    request_id: Option<String>,
    #[serde(default)]
    translated_list: Vec<Item>,
}

#[derive(Deserialize)]
struct Item {
    code: serde_json::Value,
    index: String,
    #[serde(default)]
    translated: String,
}

impl Aliyun {
    pub fn new(credentials: Credentials) -> Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(12))
            .redirect(Policy::none())
            .build()?;
        Ok(Self {
            client,
            credentials,
        })
    }

    pub fn validate(&self, target: &str) -> Result<()> {
        // Use a different source language when the user wants English.
        let (source, sample) = if target == "en" {
            ("zh", "你好")
        } else {
            ("en", "Hello")
        };
        let translations = self
            .translate(source, target, std::slice::from_ref(&sample))
            .with_context(|| format!("Aliyun translation check failed ({source} -> {target})"))?;
        ensure!(
            translations.into_iter().next().flatten().is_some(),
            "Aliyun did not return a translation for the test sentence"
        );
        Ok(())
    }

    pub fn translate(
        &self,
        source: &str,
        target: &str,
        texts: &[&str],
    ) -> Result<Vec<Option<String>>> {
        let indexed: BTreeMap<_, _> = texts
            .iter()
            .enumerate()
            .map(|(i, text)| (i, *text))
            .collect();
        let body = [
            ("ApiType", "translate_standard"),
            ("FormatType", "text"),
            ("Scene", "general"),
            ("SourceLanguage", source),
            ("SourceText", &serde_json::to_string(&indexed)?),
            ("TargetLanguage", target),
        ]
        .map(|(name, value)| format!("{name}={}", encode(value)))
        .join("&");

        let mut headers = BTreeMap::from([
            (
                "content-type",
                "application/x-www-form-urlencoded".to_owned(),
            ),
            ("host", HOST.to_owned()),
            ("x-acs-action", "GetBatchTranslate".to_owned()),
            (
                "x-acs-content-sha256",
                format!("{:x}", Sha256::digest(&body)),
            ),
            (
                "x-acs-date",
                Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
            ),
            ("x-acs-signature-nonce", Uuid::new_v4().to_string()),
            ("x-acs-version", "2018-10-12".to_owned()),
        ]);
        let authorization = authorization(&headers, &self.credentials)?;
        headers.insert("authorization", authorization);

        let mut request = self.client.post(format!("https://{HOST}/")).body(body);
        for (name, value) in headers {
            request = request.header(name, value);
        }
        let response = request
            .send()
            .context("could not reach Aliyun Machine Translation")?;
        let status = response.status();
        let response: Response = response
            .json()
            .with_context(|| format!("unexpected Aliyun response (HTTP {status})"))?;
        if !status.is_success() || !success(&response.code) {
            let hint = match response.code.to_string().trim_matches('"') {
                "10005" => "check the target language code",
                "NoPermission" | "System.SubNotPermission" | "10009" | "10011" => {
                    "grant alimt:GetBatchTranslate to the RAM user that owns this AccessKey"
                }
                "10010" | "10013" => {
                    "check that Machine Translation is enabled and the account is in good standing"
                }
                _ if status.as_u16() == 429 => "rate limited, retry later",
                _ if matches!(status.as_u16(), 401 | 403) => {
                    "check the AccessKey and RAM permissions"
                }
                _ => "check configuration or retry",
            };
            bail!(
                "Aliyun HTTP {status}, code {}, request {}: {hint}",
                safe_token(&response.code.to_string()),
                safe_token(response.request_id.as_deref().unwrap_or_default())
            );
        }
        Ok(index_results(texts.len(), response.translated_list))
    }
}

fn authorization(headers: &BTreeMap<&str, String>, credentials: &Credentials) -> Result<String> {
    let canonical_headers: String = headers
        .iter()
        .map(|(name, value)| format!("{name}:{}\n", value.trim()))
        .collect();
    let signed_headers = headers.keys().copied().collect::<Vec<_>>().join(";");
    let canonical = format!(
        "POST\n/\n\n{canonical_headers}\n{signed_headers}\n{}",
        headers["x-acs-content-sha256"]
    );
    let to_sign = format!("ACS3-HMAC-SHA256\n{:x}", Sha256::digest(canonical));
    let mut mac = Hmac::<Sha256>::new_from_slice(credentials.access_key_secret.as_bytes())?;
    mac.update(to_sign.as_bytes());
    let signature = format!("{:x}", mac.finalize().into_bytes());
    Ok(format!(
        "ACS3-HMAC-SHA256 Credential={},SignedHeaders={signed_headers},Signature={signature}",
        credentials.access_key_id
    ))
}

fn encode(value: &str) -> String {
    use std::fmt::Write;
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

fn success(code: &serde_json::Value) -> bool {
    code.as_u64() == Some(200) || code.as_str() == Some("200")
}

fn safe_token(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_ascii_alphanumeric() || ".-_".contains(*c))
        .take(80)
        .collect()
}

fn index_results(len: usize, items: Vec<Item>) -> Vec<Option<String>> {
    let mut results = vec![None; len];
    for item in items {
        if let Ok(index) = item.index.parse::<usize>()
            && let Some(result) = results.get_mut(index)
            && success(&item.code)
            && !item.translated.trim().is_empty()
        {
            *result = Some(item.translated);
        }
    }
    results
}

pub fn chunk(text: &str) -> Vec<&str> {
    const MAX_CHARS: usize = 1000;
    let mut chunks = Vec::new();
    let mut remaining = text;
    while let Some((limit, _)) = remaining.char_indices().nth(MAX_CHARS) {
        let split = remaining[..limit]
            .rfind(' ')
            .filter(|position| *position > 0)
            .unwrap_or(limit);
        chunks.push(&remaining[..split]);
        remaining = remaining[split..].trim_start();
    }
    if !remaining.is_empty() {
        chunks.push(remaining);
    }
    chunks
}

pub fn batch_size<'a>(chunks: impl Iterator<Item = &'a str>) -> usize {
    let mut characters = 0;
    chunks
        .take(50)
        .take_while(|text| {
            characters += text.chars().count();
            characters <= 8000
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_unicode_and_obeys_batch_limits() {
        let text = "测".repeat(8500);
        let chunks = chunk(&text);
        assert_eq!(chunks.concat(), text);
        assert!(chunks.iter().all(|chunk| chunk.chars().count() <= 1000));
        assert_eq!(batch_size(chunks.iter().copied()), 8);
        assert_eq!(batch_size(std::iter::repeat_n("a", 51)), 50);
    }

    #[test]
    fn results_keep_request_order_and_individual_failures() {
        let items = serde_json::from_str(
            r#"[
            {"index":"2","code":"200","translated":"third"},
            {"index":"1","code":"10007"},
            {"index":"0","code":200,"translated":"first"}
        ]"#,
        )
        .unwrap();
        assert_eq!(
            index_results(3, items),
            vec![Some("first".into()), None, Some("third".into())]
        );
        assert_eq!(encode("汉字 +~"), "%E6%B1%89%E5%AD%97%20%2B~");
    }
}
