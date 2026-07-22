use crate::config::get_metashrew;
use crate::modules::essentials::storage::EssentialsProvider;
use crate::modules::essentials::utils::inspections::resolve_contract_wasm_source;
use crate::schemas::SchemaAlkaneId;
use anyhow::{Context, Result};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlkabiFormat {
    Json,
    TypeScript,
}

impl AlkabiFormat {
    pub fn parse(raw: Option<&str>) -> Option<Self> {
        match raw?.trim().to_ascii_lowercase().as_str() {
            "json" => Some(Self::Json),
            "ts" => Some(Self::TypeScript),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::TypeScript => "ts",
        }
    }

    pub fn render(self, abi: &alkabi::AlkabiAbi) -> Result<Value> {
        match self {
            Self::Json => serde_json::from_str(&abi.to_json()).context("decode Alkabi JSON"),
            Self::TypeScript => Ok(Value::String(abi.to_ts())),
        }
    }
}

pub fn extract_contract_alkabi(
    provider: &EssentialsProvider,
    alkane: &SchemaAlkaneId,
) -> Result<alkabi::AlkabiAbi> {
    let source = resolve_contract_wasm_source(alkane, provider).unwrap_or(*alkane);
    let (wasm, _) = get_metashrew()
        .get_alkane_wasm_bytes_prefer_first_version(&source)?
        .context("contract wasm not found")?;
    alkabi::extract::extract_abi(&wasm).context("extract Alkabi ABI")
}

#[cfg(test)]
mod tests {
    use super::AlkabiFormat;
    use alkabi::extract::extract_abi;

    #[test]
    fn bundled_factory_wasm_renders_json_and_typescript() {
        let wasm = include_bytes!("../../../../test_data/factory.wasm");
        let abi = extract_abi(wasm).expect("extract Alkabi ABI");
        let json = AlkabiFormat::Json.render(&abi).expect("render Alkabi JSON");
        let typescript = AlkabiFormat::TypeScript
            .render(&abi)
            .expect("render Alkabi TypeScript")
            .as_str()
            .expect("TypeScript string")
            .to_string();

        assert_eq!(json["contract"], abi.contract);
        assert!(json["methods"].as_array().is_some_and(|methods| !methods.is_empty()));
        assert!(typescript.contains(&format!("export const {}Abi", abi.contract)));
    }

    #[test]
    fn output_format_accepts_only_json_and_ts() {
        assert_eq!(AlkabiFormat::parse(Some("JSON")), Some(AlkabiFormat::Json));
        assert_eq!(AlkabiFormat::parse(Some(" ts ")), Some(AlkabiFormat::TypeScript));
        assert_eq!(AlkabiFormat::parse(Some("typescript")), None);
        assert_eq!(AlkabiFormat::parse(None), None);
    }
}
