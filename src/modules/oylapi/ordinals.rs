use anyhow::Result;
use futures::stream::{FuturesUnordered, StreamExt};
use reqwest::Client;
use serde_json::Value;
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct OrdOutput {
    pub inscriptions: Vec<String>,
    pub runes: Value,
}

impl Default for OrdOutput {
    fn default() -> Self {
        Self { inscriptions: Vec::new(), runes: Value::Object(Default::default()) }
    }
}

pub async fn fetch_ord_outputs(
    client: &Client,
    base_url: &str,
    outpoints: &[String],
) -> Result<HashMap<String, OrdOutput>> {
    let base = base_url.trim_end_matches('/');
    let mut futures = FuturesUnordered::new();

    for outpoint in outpoints {
        let url = format!("{base}/output/{outpoint}");
        let op = outpoint.clone();
        let client = client.clone();
        futures.push(async move {
            let resp = client.get(&url).send().await;
            let value = match resp {
                Ok(r) => r.json::<Value>().await.ok(),
                Err(_) => None,
            };
            let ord = value.as_ref().map(parse_ord_output).unwrap_or_else(OrdOutput::default);
            (op, ord)
        });
    }

    let mut map = HashMap::new();
    while let Some((outpoint, ord)) = futures.next().await {
        map.insert(outpoint, ord);
    }

    Ok(map)
}

fn parse_ord_output(value: &Value) -> OrdOutput {
    let inscriptions = value
        .get("inscriptions")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect::<Vec<String>>()
        })
        .unwrap_or_default();
    let runes = match value.get("runes") {
        Some(Value::Null) => Value::Object(Default::default()),
        Some(v) => v.clone(),
        None => Value::Object(Default::default()),
    };
    OrdOutput { inscriptions, runes }
}
