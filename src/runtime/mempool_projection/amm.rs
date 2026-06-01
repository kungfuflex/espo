use super::{
    MempoolContractRule, add_to_sheet, alkane_id_from_parts, cellpack_from_protostone,
    remove_from_sheet,
};
use crate::config::{get_espo_db, get_network};
use crate::modules::ammdata::consts::get_amm_contract;
use crate::modules::ammdata::schemas::SchemaPoolSnapshot;
use crate::modules::ammdata::storage::{
    AmmDataProvider, GetPoolLpSupplyLatestParams, GetReservesSnapshotParams,
};
use crate::modules::essentials::storage::EssentialsProvider;
use crate::modules::essentials::utils::balances::{
    ContractProjection, ContractProjectionContext, ProjectionSheet,
};
use crate::runtime::mdb::Mdb;
use crate::runtime::state_at::StateAt;
use crate::schemas::SchemaAlkaneId;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

const AMM_POOL_SWAP_OPCODE: u128 = 3;
const AMM_POOL_ADD_LIQUIDITY_OPCODE: u128 = 1;
const AMM_POOL_WITHDRAW_AND_BURN_OPCODE: u128 = 2;
const AMM_POOL_COLLECT_FEES_OPCODE: u128 = 10;
const AMM_POOL_GET_TOTAL_FEE_OPCODE: u128 = 20;
const AMM_POOL_SET_TOTAL_FEE_OPCODE: u128 = 21;
const AMM_POOL_FORWARD_INCOMING_OPCODE: u128 = 50;
const AMM_POOL_GET_RESERVES_OPCODE: u128 = 97;
const AMM_POOL_GET_PRICE_CUMULATIVE_LAST_OPCODE: u128 = 98;
const AMM_POOL_GET_NAME_OPCODE: u128 = 99;
const AMM_POOL_DETAILS_OPCODE: u128 = 999;
const AMM_FACTORY_CREATE_NEW_POOL_OPCODE: u128 = 1;
const AMM_FACTORY_FIND_EXISTING_POOL_OPCODE: u128 = 2;
const AMM_FACTORY_GET_ALL_POOLS_OPCODE: u128 = 3;
const AMM_FACTORY_GET_NUM_POOLS_OPCODE: u128 = 4;
const AMM_FACTORY_COLLECT_FEES_OPCODE: u128 = 10;
const AMM_FACTORY_ADD_LIQUIDITY_OPCODE: u128 = 11;
const AMM_FACTORY_BURN_OPCODE: u128 = 12;
const AMM_FACTORY_SWAP_EXACT_IN_OPCODE: u128 = 13;
const AMM_FACTORY_SWAP_EXACT_OUT_OPCODE: u128 = 14;
const AMM_FACTORY_SET_TOTAL_FEE_FOR_POOL_OPCODE: u128 = 21;
const AMM_FACTORY_SWAP_EXACT_IN_IMPLICIT_OPCODE: u128 = 29;
const AMM_FACTORY_FORWARD_OPCODE: u128 = 50;
const DEFAULT_TOTAL_FEE_PER_1000: u128 = 10;
const FEE_DENOMINATOR: u128 = 1000;
const MINIMUM_LIQUIDITY: u128 = 1000;

pub(crate) struct AmmProjectionRule {
    factory_id: Option<SchemaAlkaneId>,
    reserves: HashMap<SchemaAlkaneId, SchemaPoolSnapshot>,
    lp_supplies: HashMap<SchemaAlkaneId, u128>,
    pools_by_pair: HashMap<(SchemaAlkaneId, SchemaAlkaneId), SchemaAlkaneId>,
}

impl AmmProjectionRule {
    pub(crate) fn new() -> Self {
        let provider = amm_provider();
        let reserves = provider
            .get_reserves_snapshot(GetReservesSnapshotParams { blockhash: StateAt::Latest })
            .ok()
            .and_then(|result| result.snapshot)
            .unwrap_or_default();
        let lp_supplies = reserves
            .keys()
            .filter_map(|pool| {
                provider
                    .get_pool_lp_supply_latest(GetPoolLpSupplyLatestParams {
                        blockhash: StateAt::Latest,
                        pool: *pool,
                    })
                    .ok()
                    .map(|result| (*pool, result.supply))
            })
            .collect();
        Self {
            factory_id: get_amm_contract(get_network()).ok(),
            pools_by_pair: pools_by_pair(&reserves),
            reserves,
            lp_supplies,
        }
    }
}

impl MempoolContractRule for AmmProjectionRule {
    fn project(&mut self, ctx: &ContractProjectionContext<'_>) -> Option<ContractProjection> {
        let cellpack = cellpack_from_protostone(ctx.protostone)?;
        let target = alkane_id_from_parts(cellpack.target.block, cellpack.target.tx)?;
        let opcode = cellpack.inputs.first().copied()?;

        if self.reserves.contains_key(&target) {
            return self.project_pool_call(target, opcode, &cellpack.inputs, ctx.incoming);
        }

        if Some(target) == self.factory_id {
            return self.project_factory_swap(opcode, &cellpack.inputs, ctx.incoming);
        }

        None
    }
}

impl AmmProjectionRule {
    fn project_pool_call(
        &mut self,
        pool_id: SchemaAlkaneId,
        opcode: u128,
        inputs: &[u128],
        incoming: &ProjectionSheet,
    ) -> Option<ContractProjection> {
        match opcode {
            AMM_POOL_ADD_LIQUIDITY_OPCODE => {
                self.project_direct_pool_add_liquidity(pool_id, incoming)
            }
            AMM_POOL_WITHDRAW_AND_BURN_OPCODE => {
                self.project_pool_burn(pool_id, None, None, None, incoming)
            }
            AMM_POOL_SWAP_OPCODE => self.project_pool_swap(pool_id, inputs, incoming),
            AMM_POOL_FORWARD_INCOMING_OPCODE
            | AMM_POOL_COLLECT_FEES_OPCODE
            | AMM_POOL_GET_TOTAL_FEE_OPCODE
            | AMM_POOL_GET_RESERVES_OPCODE
            | AMM_POOL_GET_PRICE_CUMULATIVE_LAST_OPCODE
            | AMM_POOL_GET_NAME_OPCODE
            | AMM_POOL_DETAILS_OPCODE => Some(ContractProjection { output: incoming.clone() }),
            AMM_POOL_SET_TOTAL_FEE_OPCODE => Some(ContractProjection { output: incoming.clone() }),
            _ => None,
        }
    }

    fn project_pool_swap(
        &mut self,
        pool_id: SchemaAlkaneId,
        inputs: &[u128],
        incoming: &ProjectionSheet,
    ) -> Option<ContractProjection> {
        let snapshot = self.reserves.get(&pool_id)?.clone();
        let amount_0_out = inputs.get(1).copied().unwrap_or_default();
        let amount_1_out = inputs.get(2).copied().unwrap_or_default();
        if amount_0_out > 0 && amount_1_out > 0 {
            return None;
        }

        let base_in = incoming.get(&snapshot.base_id).copied().unwrap_or_default();
        let quote_in = incoming.get(&snapshot.quote_id).copied().unwrap_or_default();
        if (base_in > 0) == (quote_in > 0) {
            return None;
        }

        let mut trial = self.reserves.clone();
        let (token_in, amount_in, token_out, expected_out) = if base_in > 0 {
            let computed = amount_out_for_pair(
                snapshot.base_id,
                snapshot.quote_id,
                base_in,
                &trial,
                &self.pools_by_pair,
            )?;
            let out = if amount_1_out > 0 { amount_1_out.min(computed) } else { computed };
            (snapshot.base_id, base_in, snapshot.quote_id, out)
        } else {
            let computed = amount_out_for_pair(
                snapshot.quote_id,
                snapshot.base_id,
                quote_in,
                &trial,
                &self.pools_by_pair,
            )?;
            let out = if amount_0_out > 0 { amount_0_out.min(computed) } else { computed };
            (snapshot.quote_id, quote_in, snapshot.base_id, out)
        };
        if expected_out == 0 {
            return None;
        }
        apply_hop(&mut trial, &self.pools_by_pair, token_in, token_out, amount_in, expected_out)?;

        self.reserves = trial;
        let mut output = incoming.clone();
        remove_from_sheet(&mut output, token_in, amount_in);
        add_to_sheet(&mut output, token_out, expected_out);
        Some(ContractProjection { output })
    }

    fn project_factory_swap(
        &mut self,
        opcode: u128,
        inputs: &[u128],
        incoming: &ProjectionSheet,
    ) -> Option<ContractProjection> {
        match opcode {
            AMM_FACTORY_ADD_LIQUIDITY_OPCODE => {
                self.project_factory_add_liquidity(inputs, incoming)
            }
            AMM_FACTORY_BURN_OPCODE => self.project_factory_burn(inputs, incoming),
            AMM_FACTORY_SWAP_EXACT_IN_OPCODE => {
                self.project_factory_exact_in(inputs, incoming, false)
            }
            AMM_FACTORY_SWAP_EXACT_IN_IMPLICIT_OPCODE => {
                self.project_factory_exact_in(inputs, incoming, true)
            }
            AMM_FACTORY_SWAP_EXACT_OUT_OPCODE => self.project_factory_exact_out(inputs, incoming),
            AMM_FACTORY_FORWARD_OPCODE
            | AMM_FACTORY_FIND_EXISTING_POOL_OPCODE
            | AMM_FACTORY_GET_ALL_POOLS_OPCODE
            | AMM_FACTORY_GET_NUM_POOLS_OPCODE
            | AMM_FACTORY_COLLECT_FEES_OPCODE => {
                Some(ContractProjection { output: incoming.clone() })
            }
            AMM_FACTORY_SET_TOTAL_FEE_FOR_POOL_OPCODE => {
                Some(ContractProjection { output: incoming.clone() })
            }
            AMM_FACTORY_CREATE_NEW_POOL_OPCODE => None,
            _ => None,
        }
    }

    fn project_factory_add_liquidity(
        &mut self,
        inputs: &[u128],
        incoming: &ProjectionSheet,
    ) -> Option<ContractProjection> {
        let token_a = parse_alkane_at(inputs, 1)?;
        let token_b = parse_alkane_at(inputs, 3)?;
        let amount_a_desired = inputs.get(5).copied().unwrap_or_default();
        let amount_b_desired = inputs.get(6).copied().unwrap_or_default();
        let amount_a_min = inputs.get(7).copied().unwrap_or_default();
        let amount_b_min = inputs.get(8).copied().unwrap_or_default();
        if amount_a_desired == 0 || amount_b_desired == 0 {
            return None;
        }

        let pool_id = *self.pools_by_pair.get(&(token_a, token_b))?;
        let snapshot = self.reserves.get(&pool_id)?.clone();
        let (reserve_a, reserve_b) = ordered_reserves(&snapshot, token_a, token_b)?;
        let (amount_a, amount_b) = optimal_liquidity_amounts(
            reserve_a,
            reserve_b,
            amount_a_desired,
            amount_b_desired,
            amount_a_min,
            amount_b_min,
        )?;
        if incoming.get(&token_a).copied().unwrap_or_default() < amount_a
            || incoming.get(&token_b).copied().unwrap_or_default() < amount_b
        {
            return None;
        }
        self.project_add_liquidity(pool_id, token_a, token_b, amount_a, amount_b, incoming)
    }

    fn project_direct_pool_add_liquidity(
        &mut self,
        pool_id: SchemaAlkaneId,
        incoming: &ProjectionSheet,
    ) -> Option<ContractProjection> {
        let snapshot = self.reserves.get(&pool_id)?.clone();
        let amount_base = incoming.get(&snapshot.base_id).copied().unwrap_or_default();
        let amount_quote = incoming.get(&snapshot.quote_id).copied().unwrap_or_default();
        if amount_base == 0 || amount_quote == 0 {
            return None;
        }
        if incoming
            .iter()
            .any(|(id, amount)| *amount > 0 && *id != snapshot.base_id && *id != snapshot.quote_id)
        {
            return None;
        }
        self.project_add_liquidity(
            pool_id,
            snapshot.base_id,
            snapshot.quote_id,
            amount_base,
            amount_quote,
            incoming,
        )
    }

    fn project_add_liquidity(
        &mut self,
        pool_id: SchemaAlkaneId,
        token_a: SchemaAlkaneId,
        token_b: SchemaAlkaneId,
        amount_a: u128,
        amount_b: u128,
        incoming: &ProjectionSheet,
    ) -> Option<ContractProjection> {
        if amount_a == 0 || amount_b == 0 {
            return None;
        }
        let snapshot = self.reserves.get(&pool_id)?.clone();
        let supply = self.lp_supply(pool_id);
        let (reserve_a, reserve_b) = ordered_reserves(&snapshot, token_a, token_b)?;
        let liquidity = minted_liquidity(supply, reserve_a, reserve_b, amount_a, amount_b)?;
        if liquidity == 0 {
            return None;
        }

        self.apply_reserve_delta(pool_id, token_a, token_b, amount_a, amount_b)?;
        *self.lp_supplies.entry(pool_id).or_default() = supply.saturating_add(liquidity);

        let mut output = incoming.clone();
        remove_from_sheet(&mut output, token_a, amount_a);
        remove_from_sheet(&mut output, token_b, amount_b);
        add_to_sheet(&mut output, pool_id, liquidity);
        Some(ContractProjection { output })
    }

    fn project_factory_burn(
        &mut self,
        inputs: &[u128],
        incoming: &ProjectionSheet,
    ) -> Option<ContractProjection> {
        let token_a = parse_alkane_at(inputs, 1)?;
        let token_b = parse_alkane_at(inputs, 3)?;
        let liquidity = inputs.get(5).copied().unwrap_or_default();
        let amount_a_min = inputs.get(6).copied().unwrap_or_default();
        let amount_b_min = inputs.get(7).copied().unwrap_or_default();
        let pool_id = *self.pools_by_pair.get(&(token_a, token_b))?;
        let available = incoming.get(&pool_id).copied().unwrap_or_default();
        if liquidity == 0 || available < liquidity {
            return None;
        }
        self.project_pool_burn(
            pool_id,
            Some(liquidity),
            Some((token_a, amount_a_min)),
            Some((token_b, amount_b_min)),
            incoming,
        )
    }

    fn project_pool_burn(
        &mut self,
        pool_id: SchemaAlkaneId,
        liquidity_override: Option<u128>,
        min_a: Option<(SchemaAlkaneId, u128)>,
        min_b: Option<(SchemaAlkaneId, u128)>,
        incoming: &ProjectionSheet,
    ) -> Option<ContractProjection> {
        let available = incoming.get(&pool_id).copied().unwrap_or_default();
        let liquidity = liquidity_override.unwrap_or(available);
        if liquidity == 0 {
            return None;
        }
        if available < liquidity {
            return None;
        }
        let snapshot = self.reserves.get(&pool_id)?.clone();
        let supply = self.lp_supply(pool_id);
        let (amount_base, amount_quote) = burned_liquidity_amounts(
            liquidity,
            snapshot.base_reserve,
            snapshot.quote_reserve,
            supply,
        )?;
        if amount_base == 0 || amount_quote == 0 {
            return None;
        }
        if let Some((id, min_amount)) = min_a {
            let actual = if id == snapshot.base_id {
                amount_base
            } else if id == snapshot.quote_id {
                amount_quote
            } else {
                return None;
            };
            if actual < min_amount {
                return None;
            }
        }
        if let Some((id, min_amount)) = min_b {
            let actual = if id == snapshot.base_id {
                amount_base
            } else if id == snapshot.quote_id {
                amount_quote
            } else {
                return None;
            };
            if actual < min_amount {
                return None;
            }
        }

        {
            let reserves = self.reserves.get_mut(&pool_id)?;
            reserves.base_reserve = reserves.base_reserve.checked_sub(amount_base)?;
            reserves.quote_reserve = reserves.quote_reserve.checked_sub(amount_quote)?;
        }
        *self.lp_supplies.entry(pool_id).or_default() = supply.checked_sub(liquidity)?;

        let mut output = incoming.clone();
        remove_from_sheet(&mut output, pool_id, liquidity);
        add_to_sheet(&mut output, snapshot.base_id, amount_base);
        add_to_sheet(&mut output, snapshot.quote_id, amount_quote);
        Some(ContractProjection { output })
    }

    fn lp_supply(&self, pool_id: SchemaAlkaneId) -> u128 {
        self.lp_supplies.get(&pool_id).copied().unwrap_or_default()
    }

    fn apply_reserve_delta(
        &mut self,
        pool_id: SchemaAlkaneId,
        token_a: SchemaAlkaneId,
        token_b: SchemaAlkaneId,
        amount_a: u128,
        amount_b: u128,
    ) -> Option<()> {
        let snapshot = self.reserves.get_mut(&pool_id)?;
        if snapshot.base_id == token_a && snapshot.quote_id == token_b {
            snapshot.base_reserve = snapshot.base_reserve.saturating_add(amount_a);
            snapshot.quote_reserve = snapshot.quote_reserve.saturating_add(amount_b);
            Some(())
        } else if snapshot.base_id == token_b && snapshot.quote_id == token_a {
            snapshot.base_reserve = snapshot.base_reserve.saturating_add(amount_b);
            snapshot.quote_reserve = snapshot.quote_reserve.saturating_add(amount_a);
            Some(())
        } else {
            None
        }
    }

    fn project_factory_exact_in(
        &mut self,
        inputs: &[u128],
        incoming: &ProjectionSheet,
        implicit: bool,
    ) -> Option<ContractProjection> {
        let (path, cursor) = parse_path(inputs)?;
        let available = incoming.get(path.first()?).copied().unwrap_or_default();
        let amount_in =
            if implicit { available } else { inputs.get(cursor).copied().unwrap_or_default() };
        if amount_in == 0 || available < amount_in {
            return None;
        }
        let min_out_index = if implicit { cursor } else { cursor + 1 };
        let min_out = inputs.get(min_out_index).copied().unwrap_or_default();

        let mut trial = self.reserves.clone();
        let mut amount = amount_in;
        for pair in path.windows(2) {
            amount = exact_in_hop(&mut trial, &self.pools_by_pair, pair[0], pair[1], amount)?;
        }
        if amount < min_out {
            return None;
        }

        self.reserves = trial;
        let mut output = incoming.clone();
        remove_from_sheet(&mut output, *path.first()?, amount_in);
        add_to_sheet(&mut output, *path.last()?, amount);
        Some(ContractProjection { output })
    }

    fn project_factory_exact_out(
        &mut self,
        inputs: &[u128],
        incoming: &ProjectionSheet,
    ) -> Option<ContractProjection> {
        let (path, cursor) = parse_path(inputs)?;
        let desired_out = inputs.get(cursor).copied().unwrap_or_default();
        let amount_in_max = inputs.get(cursor + 1).copied().unwrap_or_default();
        if desired_out == 0 {
            return None;
        }

        let mut amounts = vec![0u128; path.len()];
        *amounts.last_mut()? = desired_out;
        for i in (0..path.len() - 1).rev() {
            amounts[i] = amount_in_for_pair(
                path[i],
                path[i + 1],
                amounts[i + 1],
                &self.reserves,
                &self.pools_by_pair,
            )?;
        }
        let amount_in = amounts[0];
        let available = incoming.get(path.first()?).copied().unwrap_or_default();
        if amount_in == 0 || amount_in > amount_in_max || available < amount_in {
            return None;
        }

        let mut trial = self.reserves.clone();
        for (idx, pair) in path.windows(2).enumerate() {
            apply_hop(
                &mut trial,
                &self.pools_by_pair,
                pair[0],
                pair[1],
                amounts[idx],
                amounts[idx + 1],
            )?;
        }

        self.reserves = trial;
        let mut output = incoming.clone();
        remove_from_sheet(&mut output, *path.first()?, amount_in);
        add_to_sheet(&mut output, *path.last()?, desired_out);
        Some(ContractProjection { output })
    }
}

fn amm_provider() -> &'static AmmDataProvider {
    static PROVIDER: OnceLock<AmmDataProvider> = OnceLock::new();
    PROVIDER.get_or_init(|| {
        let db = get_espo_db();
        let amm_mdb = Arc::new(Mdb::from_db(Arc::clone(&db), b"ammdata:"));
        let essentials_mdb = Arc::new(Mdb::from_db(db, b"essentials:"));
        AmmDataProvider::new(amm_mdb, Arc::new(EssentialsProvider::new(essentials_mdb)))
    })
}

fn pools_by_pair(
    reserves: &HashMap<SchemaAlkaneId, SchemaPoolSnapshot>,
) -> HashMap<(SchemaAlkaneId, SchemaAlkaneId), SchemaAlkaneId> {
    let mut out = HashMap::new();
    for (pool, snapshot) in reserves {
        out.insert((snapshot.base_id, snapshot.quote_id), *pool);
        out.insert((snapshot.quote_id, snapshot.base_id), *pool);
    }
    out
}

fn parse_path(inputs: &[u128]) -> Option<(Vec<SchemaAlkaneId>, usize)> {
    let len = usize::try_from(*inputs.get(1)?).ok()?;
    if len < 2 {
        return None;
    }
    let mut path = Vec::with_capacity(len);
    let mut cursor = 2usize;
    for _ in 0..len {
        let id = alkane_id_from_parts(*inputs.get(cursor)?, *inputs.get(cursor + 1)?)?;
        path.push(id);
        cursor += 2;
    }
    Some((path, cursor))
}

fn parse_alkane_at(inputs: &[u128], offset: usize) -> Option<SchemaAlkaneId> {
    alkane_id_from_parts(*inputs.get(offset)?, *inputs.get(offset + 1)?)
}

fn optimal_liquidity_amounts(
    reserve_a: u128,
    reserve_b: u128,
    amount_a_desired: u128,
    amount_b_desired: u128,
    amount_a_min: u128,
    amount_b_min: u128,
) -> Option<(u128, u128)> {
    if reserve_a == 0 && reserve_b == 0 {
        return Some((amount_a_desired, amount_b_desired));
    }
    if reserve_a == 0 || reserve_b == 0 {
        return None;
    }

    let amount_b_optimal = amount_a_desired.checked_mul(reserve_b)?.checked_div(reserve_a)?;
    if amount_b_optimal <= amount_b_desired {
        if amount_b_optimal < amount_b_min {
            return None;
        }
        Some((amount_a_desired, amount_b_optimal))
    } else {
        let amount_a_optimal = amount_b_desired.checked_mul(reserve_a)?.checked_div(reserve_b)?;
        if amount_a_optimal > amount_a_desired || amount_a_optimal < amount_a_min {
            return None;
        }
        Some((amount_a_optimal, amount_b_desired))
    }
}

fn minted_liquidity(
    supply: u128,
    reserve_a: u128,
    reserve_b: u128,
    amount_a: u128,
    amount_b: u128,
) -> Option<u128> {
    if supply == 0 {
        let root_k = integer_sqrt(amount_a.checked_mul(amount_b)?);
        return root_k.checked_sub(MINIMUM_LIQUIDITY);
    }
    if reserve_a == 0 || reserve_b == 0 {
        return None;
    }
    let liquidity_a = amount_a.checked_mul(supply)?.checked_div(reserve_a)?;
    let liquidity_b = amount_b.checked_mul(supply)?.checked_div(reserve_b)?;
    Some(liquidity_a.min(liquidity_b))
}

fn burned_liquidity_amounts(
    liquidity: u128,
    reserve_a: u128,
    reserve_b: u128,
    supply: u128,
) -> Option<(u128, u128)> {
    if liquidity == 0 || supply == 0 {
        return None;
    }
    Some((
        liquidity.checked_mul(reserve_a)?.checked_div(supply)?,
        liquidity.checked_mul(reserve_b)?.checked_div(supply)?,
    ))
}

fn integer_sqrt(value: u128) -> u128 {
    if value < 2 {
        return value;
    }
    let mut x = value;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + value / x) / 2;
    }
    x
}

fn exact_in_hop(
    reserves: &mut HashMap<SchemaAlkaneId, SchemaPoolSnapshot>,
    pools_by_pair: &HashMap<(SchemaAlkaneId, SchemaAlkaneId), SchemaAlkaneId>,
    token_in: SchemaAlkaneId,
    token_out: SchemaAlkaneId,
    amount_in: u128,
) -> Option<u128> {
    let amount_out = amount_out_for_pair(token_in, token_out, amount_in, reserves, pools_by_pair)?;
    apply_hop(reserves, pools_by_pair, token_in, token_out, amount_in, amount_out)?;
    Some(amount_out)
}

fn amount_out_for_pair(
    token_in: SchemaAlkaneId,
    token_out: SchemaAlkaneId,
    amount_in: u128,
    reserves: &HashMap<SchemaAlkaneId, SchemaPoolSnapshot>,
    pools_by_pair: &HashMap<(SchemaAlkaneId, SchemaAlkaneId), SchemaAlkaneId>,
) -> Option<u128> {
    let pool = pools_by_pair.get(&(token_in, token_out))?;
    let snapshot = reserves.get(pool)?;
    let (reserve_in, reserve_out) = ordered_reserves(snapshot, token_in, token_out)?;
    get_amount_out(amount_in, reserve_in, reserve_out)
}

fn amount_in_for_pair(
    token_in: SchemaAlkaneId,
    token_out: SchemaAlkaneId,
    amount_out: u128,
    reserves: &HashMap<SchemaAlkaneId, SchemaPoolSnapshot>,
    pools_by_pair: &HashMap<(SchemaAlkaneId, SchemaAlkaneId), SchemaAlkaneId>,
) -> Option<u128> {
    let pool = pools_by_pair.get(&(token_in, token_out))?;
    let snapshot = reserves.get(pool)?;
    let (reserve_in, reserve_out) = ordered_reserves(snapshot, token_in, token_out)?;
    get_amount_in(amount_out, reserve_in, reserve_out)
}

fn apply_hop(
    reserves: &mut HashMap<SchemaAlkaneId, SchemaPoolSnapshot>,
    pools_by_pair: &HashMap<(SchemaAlkaneId, SchemaAlkaneId), SchemaAlkaneId>,
    token_in: SchemaAlkaneId,
    token_out: SchemaAlkaneId,
    amount_in: u128,
    amount_out: u128,
) -> Option<()> {
    if amount_in == 0 || amount_out == 0 {
        return None;
    }
    let pool = *pools_by_pair.get(&(token_in, token_out))?;
    let snapshot = reserves.get_mut(&pool)?;
    if snapshot.base_id == token_in && snapshot.quote_id == token_out {
        if amount_out >= snapshot.quote_reserve {
            return None;
        }
        snapshot.base_reserve = snapshot.base_reserve.saturating_add(amount_in);
        snapshot.quote_reserve = snapshot.quote_reserve.checked_sub(amount_out)?;
        Some(())
    } else if snapshot.quote_id == token_in && snapshot.base_id == token_out {
        if amount_out >= snapshot.base_reserve {
            return None;
        }
        snapshot.quote_reserve = snapshot.quote_reserve.saturating_add(amount_in);
        snapshot.base_reserve = snapshot.base_reserve.checked_sub(amount_out)?;
        Some(())
    } else {
        None
    }
}

fn ordered_reserves(
    snapshot: &SchemaPoolSnapshot,
    token_in: SchemaAlkaneId,
    token_out: SchemaAlkaneId,
) -> Option<(u128, u128)> {
    if snapshot.base_id == token_in && snapshot.quote_id == token_out {
        Some((snapshot.base_reserve, snapshot.quote_reserve))
    } else if snapshot.quote_id == token_in && snapshot.base_id == token_out {
        Some((snapshot.quote_reserve, snapshot.base_reserve))
    } else {
        None
    }
}

fn get_amount_out(amount_in: u128, reserve_in: u128, reserve_out: u128) -> Option<u128> {
    if amount_in == 0 || reserve_in == 0 || reserve_out == 0 {
        return None;
    }
    let amount_in_with_fee = amount_in.checked_mul(FEE_DENOMINATOR - DEFAULT_TOTAL_FEE_PER_1000)?;
    let numerator = amount_in_with_fee.checked_mul(reserve_out)?;
    let denominator = reserve_in.checked_mul(FEE_DENOMINATOR)?.checked_add(amount_in_with_fee)?;
    let out = numerator / denominator;
    (out > 0 && out < reserve_out).then_some(out)
}

fn get_amount_in(amount_out: u128, reserve_in: u128, reserve_out: u128) -> Option<u128> {
    if amount_out == 0 || reserve_in == 0 || amount_out >= reserve_out {
        return None;
    }
    let numerator = reserve_in.checked_mul(amount_out)?.checked_mul(FEE_DENOMINATOR)?;
    let denominator = reserve_out
        .checked_sub(amount_out)?
        .checked_mul(FEE_DENOMINATOR - DEFAULT_TOTAL_FEE_PER_1000)?;
    Some(numerator / denominator + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amount_out_uses_amm_default_one_percent_fee() {
        assert_eq!(get_amount_out(1_000, 10_000, 10_000), Some(900));
    }

    #[test]
    fn parse_factory_path_returns_cursor_after_path() {
        let id0 = SchemaAlkaneId { block: 2, tx: 0 };
        let id1 = SchemaAlkaneId { block: 32, tx: 0 };
        let (path, cursor) = parse_path(&[13, 2, 2, 0, 32, 0, 1000, 1, 100]).unwrap();
        assert_eq!(path, vec![id0, id1]);
        assert_eq!(cursor, 6);
    }

    #[test]
    fn add_liquidity_uses_optimal_ratio() {
        assert_eq!(optimal_liquidity_amounts(100, 200, 50, 150, 1, 1), Some((50, 100)));
        assert_eq!(optimal_liquidity_amounts(100, 200, 100, 50, 1, 1), Some((25, 50)));
    }

    #[test]
    fn liquidity_mint_and_burn_match_pool_formula() {
        assert_eq!(minted_liquidity(10_000, 100_000, 200_000, 10_000, 20_000), Some(1_000));
        assert_eq!(
            burned_liquidity_amounts(1_000, 110_000, 220_000, 11_000),
            Some((10_000, 20_000))
        );
    }
}
