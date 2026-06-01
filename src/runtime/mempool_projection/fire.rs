use super::{MempoolContractRule, add_to_sheet, alkane_id_from_parts, cellpack_from_protostone};
use crate::config::{get_espo_db, get_network};
use crate::modules::ammdata::consts::FRBTC_ALKANE_ID;
use crate::modules::essentials::storage::{
    EssentialsProvider, GetMultiValuesParams, GetRawValueParams, decode_u128_value,
};
use crate::modules::essentials::utils::balances::{
    ContractProjection, ContractProjectionContext, ProjectionSheet,
};
use crate::runtime::mdb::Mdb;
use crate::runtime::state_at::StateAt;
use crate::schemas::SchemaAlkaneId;
use bitcoin::Network;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

const FIRE_TOKEN_INITIALIZE_OPCODE: u128 = 0;
const FIRE_TOKEN_MINT_FROM_EMISSION_POOL_OPCODE: u128 = 77;
const FIRE_TOKEN_BURN_OPCODE: u128 = 88;
const FIRE_TOKEN_VIEW_OPCODE_START: u128 = 99;
const FIRE_TOKEN_VIEW_OPCODE_END: u128 = 103;

const FIRE_STAKING_INITIALIZE_OPCODE: u128 = 0;
const FIRE_STAKING_STAKE_OPCODE: u128 = 1;
const FIRE_STAKING_UNSTAKE_OPCODE: u128 = 2;
const FIRE_STAKING_CLAIM_REWARDS_OPCODE: u128 = 3;
const FIRE_STAKING_UPDATE_EPOCH_OPCODE: u128 = 5;

const FIRE_POSITION_INITIALIZE_OPCODE: u128 = 0;
const FIRE_POSITION_SET_REWARD_CHECKPOINT_OPCODE: u128 = 1;
const FIRE_POSITION_SET_PENDING_REWARDS_OPCODE: u128 = 2;

const FIRE_REDEMPTION_REDEEM_OPCODE: u128 = 1;

const FIRE_BONDING_BOND_OPCODE: u128 = 1;
const FIRE_BONDING_CLAIM_VESTED_OPCODE: u128 = 2;
const FIRE_BONDING_CLAIM_ALL_VESTED_OPCODE: u128 = 3;
const FIRE_BONDING_DEPOSIT_OPCODE: u128 = 10;

const FIRE_TREASURY_SEED_LIQUIDITY_OPCODE: u128 = 2;
const FIRE_TREASURY_CLAIM_TEAM_VESTING_OPCODE: u128 = 5;
const FIRE_TREASURY_WITHDRAW_LP_OPCODE: u128 = 6;
const FIRE_TREASURY_DEPOSIT_OPCODE: u128 = 10;
const FIRE_TREASURY_REDEEM_BACKING_OPCODE: u128 = 11;

const FIRE_DISTRIBUTOR_CONTRIBUTE_OPCODE: u128 = 1;
const FIRE_DISTRIBUTOR_CLAIM_OPCODE: u128 = 3;
const FIRE_DISTRIBUTOR_WITHDRAW_UNCLAIMED_OPCODE: u128 = 6;
const FIRE_DISTRIBUTOR_DEPOSIT_OPCODE: u128 = 10;
const FIRE_DISTRIBUTOR_WITHDRAW_CONTRIBUTIONS_OPCODE: u128 = 11;

#[cfg(test)]
const WEEK: u128 = 7 * 24 * 60 * 60;
#[cfg(test)]
const MONTH: u128 = 30 * 24 * 60 * 60;
#[cfg(test)]
const THREE_MONTHS: u128 = 90 * 24 * 60 * 60;
#[cfg(test)]
const SIX_MONTHS: u128 = 180 * 24 * 60 * 60;
#[cfg(test)]
const YEAR: u128 = 365 * 24 * 60 * 60;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FireDeployments {
    staking_beacon: SchemaAlkaneId,
    position_beacon: SchemaAlkaneId,
    fire_token: SchemaAlkaneId,
    staking_factory: SchemaAlkaneId,
    redemption: SchemaAlkaneId,
    price_oracle: SchemaAlkaneId,
    bonding: SchemaAlkaneId,
    treasury: SchemaAlkaneId,
    epoch0_staking: SchemaAlkaneId,
    epoch0_position: SchemaAlkaneId,
    diesel_frbtc_lp: SchemaAlkaneId,
    distributor: Option<SchemaAlkaneId>,
    contribution_token: Option<SchemaAlkaneId>,
}

pub(crate) struct FireProjectionRule {
    deployments: Option<FireDeployments>,
    contract_balances: HashMap<SchemaAlkaneId, ProjectionSheet>,
    total_fire_supply: Option<u128>,
}

impl FireProjectionRule {
    pub(crate) fn new() -> Self {
        let deployments = fire_deployments(get_network());
        let mut contract_balances = HashMap::new();
        let total_fire_supply = deployments.and_then(|deployments| {
            for owner in [
                deployments.bonding,
                deployments.redemption,
                deployments.treasury,
                deployments.epoch0_staking,
            ] {
                contract_balances.insert(owner, indexed_alkane_balances(owner));
            }
            if let Some(distributor) = deployments.distributor {
                contract_balances.insert(distributor, indexed_alkane_balances(distributor));
            }
            indexed_circulating_supply(deployments.fire_token)
        });
        Self { deployments, contract_balances, total_fire_supply }
    }
}

impl MempoolContractRule for FireProjectionRule {
    fn project(&mut self, ctx: &ContractProjectionContext<'_>) -> Option<ContractProjection> {
        let deployments = self.deployments?;
        let cellpack = cellpack_from_protostone(ctx.protostone)?;
        let target = alkane_id_from_parts(cellpack.target.block, cellpack.target.tx)?;
        let opcode = cellpack.inputs.first().copied()?;

        if target == deployments.fire_token {
            return self.project_fire_token(opcode, &cellpack.inputs, ctx.incoming, deployments);
        }
        if target == deployments.epoch0_staking || target == deployments.staking_factory {
            return self.project_staking(opcode, ctx.incoming, deployments);
        }
        if target == deployments.epoch0_position {
            return project_position_token(opcode, ctx.incoming, target);
        }
        if target == deployments.redemption {
            return self.project_redemption(opcode, ctx.incoming, deployments);
        }
        if target == deployments.bonding {
            return self.project_bonding(opcode, ctx.incoming, deployments);
        }
        if target == deployments.treasury {
            return self.project_treasury(opcode, &cellpack.inputs, ctx.incoming, deployments);
        }
        if Some(target) == deployments.distributor {
            return self.project_distributor(opcode, ctx.incoming, deployments);
        }
        if target == deployments.staking_beacon
            || target == deployments.position_beacon
            || target == deployments.price_oracle
        {
            return project_read_or_admin_call(opcode, ctx.incoming);
        }

        None
    }
}

impl FireProjectionRule {
    fn project_fire_token(
        &mut self,
        opcode: u128,
        inputs: &[u128],
        incoming: &ProjectionSheet,
        deployments: FireDeployments,
    ) -> Option<ContractProjection> {
        match opcode {
            FIRE_TOKEN_INITIALIZE_OPCODE => {
                let treasury_amount = inputs.get(5).copied().unwrap_or_default();
                let team_amount = inputs.get(8).copied().unwrap_or_default();
                let minted = treasury_amount.saturating_add(team_amount);
                self.total_fire_supply =
                    Some(self.total_fire_supply.unwrap_or_default().saturating_add(minted));
                self.add_contract_balance(deployments.treasury, deployments.fire_token, minted);
                let mut output = ProjectionSheet::new();
                add_to_sheet(&mut output, deployments.fire_token, minted);
                Some(ContractProjection { output })
            }
            FIRE_TOKEN_MINT_FROM_EMISSION_POOL_OPCODE => {
                let amount = inputs.get(1).copied().unwrap_or_default();
                if amount == 0 {
                    return None;
                }
                self.total_fire_supply =
                    Some(self.total_fire_supply.unwrap_or_default().saturating_add(amount));
                let mut output = incoming.clone();
                add_to_sheet(&mut output, deployments.fire_token, amount);
                Some(ContractProjection { output })
            }
            FIRE_TOKEN_BURN_OPCODE => {
                let burned = incoming.get(&deployments.fire_token).copied().unwrap_or_default();
                if burned > 0 {
                    self.total_fire_supply =
                        Some(self.total_fire_supply.unwrap_or_default().saturating_sub(burned));
                }
                Some(ContractProjection { output: without_token(incoming, deployments.fire_token) })
            }
            FIRE_TOKEN_VIEW_OPCODE_START..=FIRE_TOKEN_VIEW_OPCODE_END => passthrough(incoming),
            _ => None,
        }
    }

    fn project_staking(
        &mut self,
        opcode: u128,
        incoming: &ProjectionSheet,
        deployments: FireDeployments,
    ) -> Option<ContractProjection> {
        match opcode {
            FIRE_STAKING_INITIALIZE_OPCODE
            | FIRE_STAKING_UNSTAKE_OPCODE
            | FIRE_STAKING_CLAIM_REWARDS_OPCODE
            | FIRE_STAKING_UPDATE_EPOCH_OPCODE
            | 12
            | 14
            | 15
            | 30
            | 31
            | 36 => passthrough(incoming),
            FIRE_STAKING_STAKE_OPCODE => {
                let lp_amount =
                    incoming.get(&deployments.diesel_frbtc_lp).copied().unwrap_or_default();
                if lp_amount == 0 {
                    return None;
                }
                self.add_contract_balance(
                    deployments.epoch0_staking,
                    deployments.diesel_frbtc_lp,
                    lp_amount,
                );
                let mut output = incoming.clone();
                output.remove(&deployments.diesel_frbtc_lp);
                add_to_sheet(&mut output, deployments.epoch0_position, 1);
                Some(ContractProjection { output })
            }
            _ => None,
        }
    }

    fn project_redemption(
        &mut self,
        opcode: u128,
        incoming: &ProjectionSheet,
        deployments: FireDeployments,
    ) -> Option<ContractProjection> {
        match opcode {
            FIRE_REDEMPTION_REDEEM_OPCODE => {
                let fire_amount =
                    incoming.get(&deployments.fire_token).copied().unwrap_or_default();
                let mut output = without_token(incoming, deployments.fire_token);
                self.add_treasury_redemption_backing(&mut output, fire_amount, deployments);
                Some(ContractProjection { output })
            }
            0 | 2 | 3 | 4 | 20..=24 => passthrough(incoming),
            _ => None,
        }
    }

    fn project_bonding(
        &mut self,
        opcode: u128,
        incoming: &ProjectionSheet,
        deployments: FireDeployments,
    ) -> Option<ContractProjection> {
        match opcode {
            FIRE_BONDING_BOND_OPCODE => {
                let lp_amount =
                    incoming.get(&deployments.diesel_frbtc_lp).copied().unwrap_or_default();
                if lp_amount > 0 {
                    self.add_contract_balance(
                        deployments.treasury,
                        deployments.diesel_frbtc_lp,
                        lp_amount,
                    );
                }
                Some(ContractProjection {
                    output: without_token(incoming, deployments.diesel_frbtc_lp),
                })
            }
            FIRE_BONDING_DEPOSIT_OPCODE => {
                let fire_amount =
                    incoming.get(&deployments.fire_token).copied().unwrap_or_default();
                if fire_amount > 0 {
                    self.add_contract_balance(
                        deployments.bonding,
                        deployments.fire_token,
                        fire_amount,
                    );
                }
                Some(ContractProjection { output: without_token(incoming, deployments.fire_token) })
            }
            FIRE_BONDING_CLAIM_VESTED_OPCODE
            | FIRE_BONDING_CLAIM_ALL_VESTED_OPCODE
            | 0
            | 4
            | 5
            | 6
            | 20..=25 => passthrough(incoming),
            _ => None,
        }
    }

    fn project_treasury(
        &mut self,
        opcode: u128,
        inputs: &[u128],
        incoming: &ProjectionSheet,
        deployments: FireDeployments,
    ) -> Option<ContractProjection> {
        match opcode {
            FIRE_TREASURY_SEED_LIQUIDITY_OPCODE => {
                let amount = inputs.get(1).copied().unwrap_or_default();
                let projected = self.take_contract_balance(
                    deployments.treasury,
                    deployments.fire_token,
                    amount,
                );
                let mut output = incoming.clone();
                add_to_sheet(&mut output, deployments.fire_token, projected);
                Some(ContractProjection { output })
            }
            FIRE_TREASURY_WITHDRAW_LP_OPCODE => {
                let lp_type = inputs.get(1).copied().unwrap_or_default();
                let amount = inputs.get(2).copied().unwrap_or_default();
                let mut output = incoming.clone();
                if lp_type == 1 {
                    let projected = self.take_contract_balance(
                        deployments.treasury,
                        deployments.diesel_frbtc_lp,
                        amount,
                    );
                    add_to_sheet(&mut output, deployments.diesel_frbtc_lp, projected);
                }
                Some(ContractProjection { output })
            }
            FIRE_TREASURY_DEPOSIT_OPCODE => {
                for (token, amount) in incoming {
                    self.add_contract_balance(deployments.treasury, *token, *amount);
                }
                Some(ContractProjection { output: ProjectionSheet::new() })
            }
            FIRE_TREASURY_REDEEM_BACKING_OPCODE => {
                let fire_amount = inputs.get(1).copied().unwrap_or_default();
                let mut output = incoming.clone();
                self.add_treasury_redemption_backing(&mut output, fire_amount, deployments);
                Some(ContractProjection { output })
            }
            FIRE_TREASURY_CLAIM_TEAM_VESTING_OPCODE | 0 | 1 | 3 | 4 | 20..=23 => {
                passthrough(incoming)
            }
            _ => None,
        }
    }

    fn project_distributor(
        &mut self,
        opcode: u128,
        incoming: &ProjectionSheet,
        deployments: FireDeployments,
    ) -> Option<ContractProjection> {
        match opcode {
            FIRE_DISTRIBUTOR_CONTRIBUTE_OPCODE => {
                let token = deployments.contribution_token?;
                let amount = incoming.get(&token).copied().unwrap_or_default();
                if let Some(distributor) = deployments.distributor {
                    self.add_contract_balance(distributor, token, amount);
                }
                Some(ContractProjection { output: without_token(incoming, token) })
            }
            FIRE_DISTRIBUTOR_DEPOSIT_OPCODE => {
                let amount = incoming.get(&deployments.fire_token).copied().unwrap_or_default();
                if let Some(distributor) = deployments.distributor {
                    self.add_contract_balance(distributor, deployments.fire_token, amount);
                }
                Some(ContractProjection { output: without_token(incoming, deployments.fire_token) })
            }
            FIRE_DISTRIBUTOR_CLAIM_OPCODE
            | FIRE_DISTRIBUTOR_WITHDRAW_UNCLAIMED_OPCODE
            | FIRE_DISTRIBUTOR_WITHDRAW_CONTRIBUTIONS_OPCODE
            | 0
            | 2
            | 4
            | 5
            | 20..=25 => passthrough(incoming),
            _ => None,
        }
    }

    fn add_treasury_redemption_backing(
        &mut self,
        output: &mut ProjectionSheet,
        fire_amount: u128,
        deployments: FireDeployments,
    ) {
        if fire_amount == 0 {
            return;
        }
        let Some(total_supply) = self.total_fire_supply.filter(|supply| *supply > 0) else {
            return;
        };
        let diesel_balance =
            self.contract_balance(deployments.treasury, deployments.diesel_frbtc_lp);
        let diesel_share = diesel_balance.saturating_mul(fire_amount) / total_supply;
        let diesel_taken = self.take_contract_balance(
            deployments.treasury,
            deployments.diesel_frbtc_lp,
            diesel_share,
        );
        add_to_sheet(output, deployments.diesel_frbtc_lp, diesel_taken);
    }

    fn contract_balance(&self, owner: SchemaAlkaneId, token: SchemaAlkaneId) -> u128 {
        self.contract_balances
            .get(&owner)
            .and_then(|sheet| sheet.get(&token))
            .copied()
            .unwrap_or_default()
    }

    fn add_contract_balance(&mut self, owner: SchemaAlkaneId, token: SchemaAlkaneId, amount: u128) {
        if amount == 0 {
            return;
        }
        let sheet = self.contract_balances.entry(owner).or_default();
        add_to_sheet(sheet, token, amount);
    }

    fn take_contract_balance(
        &mut self,
        owner: SchemaAlkaneId,
        token: SchemaAlkaneId,
        amount: u128,
    ) -> u128 {
        if amount == 0 {
            return 0;
        }
        let sheet = self.contract_balances.entry(owner).or_default();
        let taken = sheet.get(&token).copied().unwrap_or_default().min(amount);
        if taken == 0 {
            return 0;
        }
        let remaining = sheet.get(&token).copied().unwrap_or_default().saturating_sub(taken);
        if remaining == 0 {
            sheet.remove(&token);
        } else {
            sheet.insert(token, remaining);
        }
        taken
    }
}

fn fire_deployments(network: Network) -> Option<FireDeployments> {
    match network {
        Network::Bitcoin => Some(FireDeployments {
            staking_beacon: id(2, 77621),
            position_beacon: id(2, 77622),
            fire_token: id(2, 77623),
            staking_factory: id(2, 77624),
            redemption: id(2, 77625),
            price_oracle: id(2, 77626),
            bonding: id(2, 77627),
            treasury: id(2, 77628),
            epoch0_staking: id(2, 77631),
            epoch0_position: id(2, 77632),
            diesel_frbtc_lp: id(2, 77087),
            distributor: None,
            contribution_token: Some(FRBTC_ALKANE_ID),
        }),
        Network::Regtest | Network::Signet | Network::Testnet | Network::Testnet4 => None,
    }
}

fn project_position_token(
    opcode: u128,
    incoming: &ProjectionSheet,
    position_token: SchemaAlkaneId,
) -> Option<ContractProjection> {
    match opcode {
        FIRE_POSITION_INITIALIZE_OPCODE => {
            let mut output = ProjectionSheet::new();
            add_to_sheet(&mut output, position_token, 1);
            Some(ContractProjection { output })
        }
        FIRE_POSITION_SET_REWARD_CHECKPOINT_OPCODE
        | FIRE_POSITION_SET_PENDING_REWARDS_OPCODE
        | 10..=17
        | 19
        | 23
        | 99
        | 100 => passthrough(incoming),
        _ => None,
    }
}

fn project_read_or_admin_call(
    opcode: u128,
    incoming: &ProjectionSheet,
) -> Option<ContractProjection> {
    match opcode {
        0..=25 | 30 | 31 | 36 | 99 | 100 => passthrough(incoming),
        _ => None,
    }
}

fn without_token(incoming: &ProjectionSheet, token: SchemaAlkaneId) -> ProjectionSheet {
    let mut output = incoming.clone();
    output.remove(&token);
    output
}

fn passthrough(incoming: &ProjectionSheet) -> Option<ContractProjection> {
    Some(ContractProjection { output: incoming.clone() })
}

fn id(block: u32, tx: u64) -> SchemaAlkaneId {
    SchemaAlkaneId { block, tx }
}

#[cfg(test)]
fn lock_multiplier(duration: u128) -> u128 {
    match duration {
        d if d >= YEAR => 300,
        d if d >= SIX_MONTHS => 250,
        d if d >= THREE_MONTHS => 200,
        d if d >= MONTH => 150,
        d if d >= WEEK => 125,
        _ => 100,
    }
}

fn essentials_provider() -> &'static EssentialsProvider {
    static PROVIDER: OnceLock<EssentialsProvider> = OnceLock::new();
    PROVIDER.get_or_init(|| {
        let db = get_espo_db();
        let essentials_mdb = Arc::new(Mdb::from_db(db, b"essentials:"));
        EssentialsProvider::new(essentials_mdb)
    })
}

fn indexed_alkane_balances(owner: SchemaAlkaneId) -> ProjectionSheet {
    let provider = essentials_provider();
    let table = provider.table();
    let len = provider
        .get_raw_value(GetRawValueParams {
            blockhash: StateAt::Latest,
            key: table.alkane_balance_list_len_key(&owner),
        })
        .ok()
        .and_then(|result| result.value)
        .and_then(|bytes| {
            if bytes.len() == 4 {
                Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
            } else {
                None
            }
        })
        .unwrap_or_default();
    if len == 0 {
        return ProjectionSheet::new();
    }

    let idx_keys = (0..len)
        .map(|idx| table.alkane_balance_list_idx_key(&owner, idx))
        .collect::<Vec<_>>();
    let idx_values = provider
        .get_multi_values(GetMultiValuesParams { blockhash: StateAt::Latest, keys: idx_keys })
        .map(|result| result.values)
        .unwrap_or_default();
    let mut tokens = Vec::new();
    let mut balance_keys = Vec::new();
    for value in idx_values.into_iter().flatten() {
        if value.len() != 12 {
            continue;
        }
        let token = SchemaAlkaneId {
            block: u32::from_be_bytes([value[0], value[1], value[2], value[3]]),
            tx: u64::from_be_bytes([
                value[4], value[5], value[6], value[7], value[8], value[9], value[10], value[11],
            ]),
        };
        tokens.push(token);
        balance_keys.push(table.alkane_balance_key(&owner, &token));
    }

    let values = provider
        .get_multi_values(GetMultiValuesParams { blockhash: StateAt::Latest, keys: balance_keys })
        .map(|result| result.values)
        .unwrap_or_default();
    let mut out = ProjectionSheet::new();
    for (token, value) in tokens.into_iter().zip(values.into_iter()) {
        let Some(bytes) = value else { continue };
        let Ok(amount) = decode_u128_value(&bytes) else { continue };
        add_to_sheet(&mut out, token, amount);
    }
    out
}

fn indexed_circulating_supply(token: SchemaAlkaneId) -> Option<u128> {
    let provider = essentials_provider();
    let table = provider.table();
    provider
        .get_raw_value(GetRawValueParams {
            blockhash: StateAt::Latest,
            key: table.circulating_supply_latest_key(&token),
        })
        .ok()
        .and_then(|result| result.value)
        .and_then(|bytes| decode_u128_value(&bytes).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mainnet_fire() -> FireDeployments {
        fire_deployments(Network::Bitcoin).unwrap()
    }

    fn test_rule(deployments: FireDeployments) -> FireProjectionRule {
        FireProjectionRule {
            deployments: Some(deployments),
            contract_balances: HashMap::new(),
            total_fire_supply: Some(1_000_000_000),
        }
    }

    #[test]
    fn mainnet_deployments_match_known_fire_ids() {
        let deployments = mainnet_fire();
        assert_eq!(deployments.fire_token, id(2, 77623));
        assert_eq!(deployments.epoch0_staking, id(2, 77631));
        assert_eq!(deployments.epoch0_position, id(2, 77632));
        assert_eq!(deployments.diesel_frbtc_lp, id(2, 77087));
    }

    #[test]
    fn stake_consumes_diesel_frbtc_lp_and_mints_position() {
        let deployments = mainnet_fire();
        let mut incoming = ProjectionSheet::new();
        add_to_sheet(&mut incoming, deployments.diesel_frbtc_lp, 42);
        add_to_sheet(&mut incoming, deployments.fire_token, 7);

        let mut rule = test_rule(deployments);
        let projection =
            rule.project_staking(FIRE_STAKING_STAKE_OPCODE, &incoming, deployments).unwrap();

        assert_eq!(projection.output.get(&deployments.diesel_frbtc_lp), None);
        assert_eq!(projection.output.get(&deployments.epoch0_position), Some(&1));
        assert_eq!(projection.output.get(&deployments.fire_token), Some(&7));
    }

    #[test]
    fn bonding_consumes_lp_and_deposit_consumes_fire() {
        let deployments = mainnet_fire();
        let mut incoming = ProjectionSheet::new();
        add_to_sheet(&mut incoming, deployments.diesel_frbtc_lp, 42);
        add_to_sheet(&mut incoming, deployments.fire_token, 7);

        let mut rule = test_rule(deployments);
        let bonded =
            rule.project_bonding(FIRE_BONDING_BOND_OPCODE, &incoming, deployments).unwrap();
        assert_eq!(bonded.output.get(&deployments.diesel_frbtc_lp), None);
        assert_eq!(bonded.output.get(&deployments.fire_token), Some(&7));

        let deposited = rule
            .project_bonding(FIRE_BONDING_DEPOSIT_OPCODE, &incoming, deployments)
            .unwrap();
        assert_eq!(deposited.output.get(&deployments.fire_token), None);
        assert_eq!(deposited.output.get(&deployments.diesel_frbtc_lp), Some(&42));
    }

    #[test]
    fn lock_multipliers_match_fire_support_tiers() {
        assert_eq!(lock_multiplier(0), 100);
        assert_eq!(lock_multiplier(WEEK), 125);
        assert_eq!(lock_multiplier(MONTH), 150);
        assert_eq!(lock_multiplier(THREE_MONTHS), 200);
        assert_eq!(lock_multiplier(SIX_MONTHS), 250);
        assert_eq!(lock_multiplier(YEAR), 300);
    }
}
