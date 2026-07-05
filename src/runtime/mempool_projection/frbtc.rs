use super::{MempoolContractRule, add_to_sheet, cellpack_from_protostone};
use crate::config::get_network;
use crate::modules::ammdata::consts::FRBTC_ALKANE_ID;
use crate::modules::essentials::utils::balances::{ContractProjection, ContractProjectionContext};
use crate::modules::subfrost::consts::get_subfrost_wrap_address;
use bitcoin::Address;
use std::str::FromStr;

const FRBTC_WRAP_OPCODE: u128 = 77;
const FRBTC_UNWRAP_OPCODE: u128 = 78;

pub(crate) struct FrBtcProjectionRule;

impl MempoolContractRule for FrBtcProjectionRule {
    fn project(&mut self, ctx: &ContractProjectionContext<'_>) -> Option<ContractProjection> {
        let cellpack = cellpack_from_protostone(ctx.protostone)?;
        if cellpack.target.block != u128::from(FRBTC_ALKANE_ID.block)
            || cellpack.target.tx != u128::from(FRBTC_ALKANE_ID.tx)
        {
            return None;
        }

        match cellpack.inputs.first().copied()? {
            FRBTC_WRAP_OPCODE => {
                let sats = wrap_btc_sats(ctx, &cellpack.inputs)?;
                let minted = wrap_mint_amount(sats);
                let mut output = ctx.incoming.clone();
                add_to_sheet(&mut output, FRBTC_ALKANE_ID, minted);
                Some(ContractProjection { output })
            }
            FRBTC_UNWRAP_OPCODE => {
                let mut output = ctx.incoming.clone();
                output.remove(&FRBTC_ALKANE_ID);
                Some(ContractProjection { output })
            }
            _ => None,
        }
    }
}

fn wrap_btc_sats(ctx: &ContractProjectionContext<'_>, inputs: &[u128]) -> Option<u128> {
    if subfrost_wrap_address_is_configured() {
        return subfrost_output_sats(ctx);
    }

    let candidates = [inputs.get(1).copied(), ctx.protostone.from.map(u128::from), Some(0)];

    for candidate in candidates.into_iter().flatten() {
        let Ok(vout) = usize::try_from(candidate) else {
            continue;
        };
        let Some(output) = ctx.tx.output.get(vout) else {
            continue;
        };
        if output.script_pubkey.is_op_return() {
            continue;
        }
        let sats = u128::from(output.value.to_sat());
        if sats > 0 {
            return Some(sats);
        }
    }

    None
}

fn wrap_mint_amount(sats: u128) -> u128 {
    sats
}

fn subfrost_output_sats(ctx: &ContractProjectionContext<'_>) -> Option<u128> {
    let network = get_network();
    let address = get_subfrost_wrap_address(network);
    let script_pubkey =
        Address::from_str(address).ok()?.require_network(network).ok()?.script_pubkey();
    ctx.tx
        .output
        .iter()
        .find(|output| output.script_pubkey == script_pubkey)
        .map(|output| u128::from(output.value.to_sat()))
        .filter(|sats| *sats > 0)
}

fn subfrost_wrap_address_is_configured() -> bool {
    !get_subfrost_wrap_address(get_network()).is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_mint_amount_matches_signer_output_sats() {
        assert_eq!(wrap_mint_amount(100_000_000), 100_000_000);
    }
}
