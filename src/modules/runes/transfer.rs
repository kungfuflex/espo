use bitcoin::Transaction;
use std::collections::BTreeMap;

pub type RuneSheet<I> = BTreeMap<I, u128>;
pub type OutputRuneSheets<I> = BTreeMap<u32, RuneSheet<I>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeftoverPolicy {
    PointerOrFirstNonOpReturn,
}

#[derive(Clone, Copy, Debug)]
pub struct TransferRules {
    pub leftover_policy: LeftoverPolicy,
}

impl Default for TransferRules {
    fn default() -> Self {
        Self { leftover_policy: LeftoverPolicy::PointerOrFirstNonOpReturn }
    }
}

pub trait RunestoneTransfer<I>
where
    I: Ord + Copy,
{
    fn apply_edict(
        &self,
        tx: &Transaction,
        unallocated: &mut RuneSheet<I>,
        outputs: &mut OutputRuneSheets<I>,
        id: I,
        amount: u128,
        output: u32,
    );

    fn route_leftovers(
        &self,
        tx: &Transaction,
        unallocated: RuneSheet<I>,
        outputs: &mut OutputRuneSheets<I>,
        pointer: Option<u32>,
    ) -> RuneSheet<I>;
}

impl<I> RunestoneTransfer<I> for TransferRules
where
    I: Ord + Copy,
{
    fn apply_edict(
        &self,
        tx: &Transaction,
        unallocated: &mut RuneSheet<I>,
        outputs: &mut OutputRuneSheets<I>,
        id: I,
        amount: u128,
        output: u32,
    ) {
        let Some(balance) = unallocated.get_mut(&id) else {
            return;
        };
        let output = output as usize;
        if output > tx.output.len() {
            return;
        }

        let mut allocate = |balance: &mut u128, amount: u128, vout: u32| {
            if amount == 0 {
                return;
            }
            *balance = balance.saturating_sub(amount);
            *outputs.entry(vout).or_default().entry(id).or_default() += amount;
        };

        if output == tx.output.len() {
            let destinations: Vec<u32> = tx
                .output
                .iter()
                .enumerate()
                .filter_map(|(vout, out)| {
                    (!out.script_pubkey.is_op_return()).then_some(vout as u32)
                })
                .collect();
            if destinations.is_empty() {
                return;
            }
            if amount == 0 {
                let each = *balance / destinations.len() as u128;
                let remainder = (*balance % destinations.len() as u128) as usize;
                for (idx, vout) in destinations.into_iter().enumerate() {
                    allocate(balance, if idx < remainder { each + 1 } else { each }, vout);
                }
            } else {
                for vout in destinations {
                    let qty = amount.min(*balance);
                    allocate(balance, qty, vout);
                }
            }
        } else {
            let qty = if amount == 0 { *balance } else { amount.min(*balance) };
            allocate(balance, qty, output as u32);
        }
    }

    fn route_leftovers(
        &self,
        tx: &Transaction,
        unallocated: RuneSheet<I>,
        outputs: &mut OutputRuneSheets<I>,
        pointer: Option<u32>,
    ) -> RuneSheet<I> {
        let destination = match self.leftover_policy {
            LeftoverPolicy::PointerOrFirstNonOpReturn => pointer.or_else(|| {
                tx.output.iter().enumerate().find_map(|(vout, out)| {
                    (!out.script_pubkey.is_op_return()).then_some(vout as u32)
                })
            }),
        };

        let Some(vout) = destination else {
            return unallocated;
        };
        if vout as usize >= tx.output.len() {
            return unallocated;
        }

        let sheet = outputs.entry(vout).or_default();
        for (id, amount) in unallocated {
            if amount > 0 {
                *sheet.entry(id).or_default() += amount;
            }
        }
        BTreeMap::new()
    }
}
