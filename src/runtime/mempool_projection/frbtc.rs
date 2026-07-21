use super::{MempoolContractRule, add_to_sheet, cellpack_from_protostone, remove_from_sheet};
use crate::config::get_espo_db;
use crate::modules::ammdata::consts::FRBTC_ALKANE_ID;
use crate::modules::essentials::storage::EssentialsProvider;
use crate::modules::essentials::utils::balances::{ContractProjection, ContractProjectionContext};
use crate::modules::subfrost::signer::get_signer_script;
use crate::runtime::mdb::Mdb;
use bitcoin::{ScriptBuf, TxOut};
use std::sync::Arc;

const FRBTC_WRAP_OPCODE: u128 = 77;
const FRBTC_UNWRAP_OPCODE: u128 = 78;

pub(crate) struct FrBtcProjectionRule {
    signer_script: Option<ScriptBuf>,
}

impl FrBtcProjectionRule {
    pub(crate) fn new() -> Self {
        let mdb = Arc::new(Mdb::from_db(get_espo_db(), b"essentials:"));
        let essentials_provider = EssentialsProvider::new(mdb);
        let signer_script = get_signer_script(&essentials_provider).ok().flatten();
        Self { signer_script }
    }
}

impl MempoolContractRule for FrBtcProjectionRule {
    fn project(&mut self, ctx: &ContractProjectionContext<'_>) -> Option<ContractProjection> {
        let cellpack = cellpack_from_protostone(ctx.protostone)?;
        if cellpack.target.block != u128::from(FRBTC_ALKANE_ID.block)
            || cellpack.target.tx != u128::from(FRBTC_ALKANE_ID.tx)
        {
            return None;
        }
        let signer_script = self.signer_script.as_ref()?;

        match cellpack.inputs.first().copied()? {
            FRBTC_WRAP_OPCODE => {
                let sats = signer_output_sats(&ctx.tx.output, signer_script)?;
                let minted = wrap_mint_amount(sats);
                let mut output = ctx.incoming.clone();
                add_to_sheet(&mut output, FRBTC_ALKANE_ID, minted);
                Some(ContractProjection { output })
            }
            FRBTC_UNWRAP_OPCODE => {
                let signer_vout = usize::try_from(*cellpack.inputs.get(1)?).ok()?;
                let amount = *cellpack.inputs.get(2)?;
                if amount == 0 || !signer_vout_matches(&ctx.tx.output, signer_vout, signer_script) {
                    return None;
                }
                let mut output = ctx.incoming.clone();
                if remove_from_sheet(&mut output, FRBTC_ALKANE_ID, amount) != amount {
                    return None;
                }
                Some(ContractProjection { output })
            }
            _ => None,
        }
    }

    fn prefer_input_projection(&self) -> bool {
        true
    }
}

fn wrap_mint_amount(sats: u128) -> u128 {
    sats
}

fn signer_output_sats(outputs: &[TxOut], signer_script: &ScriptBuf) -> Option<u128> {
    let sats = outputs
        .iter()
        .filter(|output| output.script_pubkey == *signer_script)
        .fold(0u128, |total, output| total.saturating_add(u128::from(output.value.to_sat())));
    (sats > 0).then_some(sats)
}

fn signer_vout_matches(outputs: &[TxOut], vout: usize, signer_script: &ScriptBuf) -> bool {
    outputs.get(vout).is_some_and(|output| output.script_pubkey == *signer_script)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::Amount;

    fn signer_script() -> ScriptBuf {
        ScriptBuf::from_bytes(
            hex::decode("51201d4830313fb48f68b43b07391fe1232f8488621b2cbc5fb4b26d8935e4bf1cb4")
                .unwrap(),
        )
    }

    #[test]
    fn wrap_mint_amount_matches_signer_output_sats() {
        assert_eq!(wrap_mint_amount(100_000_000), 100_000_000);
    }

    #[test]
    fn wrap_uses_signer_outputs_instead_of_vout_zero() {
        let signer = signer_script();
        let outputs = vec![
            TxOut { value: Amount::from_sat(10), script_pubkey: ScriptBuf::new() },
            TxOut { value: Amount::from_sat(50), script_pubkey: signer.clone() },
        ];
        assert_eq!(signer_output_sats(&outputs, &signer), Some(50));
    }

    #[test]
    fn unwrap_requires_the_requested_signer_vout() {
        let signer = signer_script();
        let outputs = vec![
            TxOut { value: Amount::from_sat(50), script_pubkey: ScriptBuf::new() },
            TxOut { value: Amount::from_sat(330), script_pubkey: signer.clone() },
        ];
        assert!(!signer_vout_matches(&outputs, 0, &signer));
        assert!(signer_vout_matches(&outputs, 1, &signer));
    }
}
