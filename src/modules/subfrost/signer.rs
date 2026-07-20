use crate::modules::essentials::storage::{EssentialsProvider, GetAlkaneStorageValueParams};
use crate::runtime::state_at::StateAt;
use crate::schemas::SchemaAlkaneId;
use anyhow::{Result, anyhow};
use bitcoin::{Address, Network, ScriptBuf};

pub const SIGNER_ALKANE_ID: SchemaAlkaneId = SchemaAlkaneId { block: 32, tx: 0 };
pub const SIGNER_STORAGE_KEY: &[u8] = b"/signer";

#[derive(Clone, Debug)]
pub struct SubfrostSigner {
    pub address: Address,
    pub script_pubkey: ScriptBuf,
}

pub fn get_signer(
    provider: &EssentialsProvider,
    network: Network,
) -> Result<Option<SubfrostSigner>> {
    let Some(script_pubkey) = get_signer_script(provider)? else {
        return Ok(None);
    };
    let address = Address::from_script(script_pubkey.as_script(), network)
        .map_err(|e| anyhow!("invalid /signer scriptPubKey: {e}"))?;
    Ok(Some(SubfrostSigner { address, script_pubkey }))
}

pub fn get_signer_script(provider: &EssentialsProvider) -> Result<Option<ScriptBuf>> {
    let value = provider
        .get_alkane_storage_value(GetAlkaneStorageValueParams {
            blockhash: StateAt::Latest,
            alkane: SIGNER_ALKANE_ID,
            key: SIGNER_STORAGE_KEY.to_vec(),
        })?
        .value;
    value.map(parse_signer_script).transpose()
}

fn parse_signer_script(value: Vec<u8>) -> Result<ScriptBuf> {
    let script = ScriptBuf::from_bytes(value);
    if !script.is_p2tr() {
        return Err(anyhow!("/signer is not a P2TR scriptPubKey"));
    }
    Ok(script)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIGNER_SCRIPT: &str =
        "51201d4830313fb48f68b43b07391fe1232f8488621b2cbc5fb4b26d8935e4bf1cb4";

    #[test]
    fn parses_indexed_signer_script() {
        let script = parse_signer_script(hex::decode(SIGNER_SCRIPT).unwrap()).unwrap();
        assert!(script.is_p2tr());
        let address = Address::from_script(script.as_script(), Network::Regtest).unwrap();
        assert_eq!(
            address.to_string(),
            "bcrt1pr4yrqvflkj8k3dpmquu3lcfr97zgscsm9j79ld9jdkynte9lrj6qlcsdcx"
        );
    }

    #[test]
    fn rejects_raw_keys_and_non_p2tr_scripts() {
        assert!(parse_signer_script(vec![0; 32]).is_err());
        let mut p2wpkh = vec![0x00, 0x14];
        p2wpkh.extend([0; 20]);
        assert!(parse_signer_script(p2wpkh).is_err());
    }
}
