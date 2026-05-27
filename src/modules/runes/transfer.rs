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

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::{Amount, ScriptBuf, TxOut, locktime::absolute, opcodes, transaction};

    fn tx_with_middle_op_return() -> Transaction {
        Transaction {
            version: transaction::Version::TWO,
            lock_time: absolute::LockTime::ZERO,
            input: vec![],
            output: vec![
                TxOut { value: Amount::ZERO, script_pubkey: ScriptBuf::new() },
                TxOut {
                    value: Amount::ZERO,
                    script_pubkey: ScriptBuf::builder()
                        .push_opcode(opcodes::all::OP_RETURN)
                        .into_script(),
                },
                TxOut { value: Amount::ZERO, script_pubkey: ScriptBuf::new() },
            ],
        }
    }

    fn output_amount(outputs: &OutputRuneSheets<u32>, vout: u32, id: u32) -> u128 {
        outputs.get(&vout).and_then(|sheet| sheet.get(&id)).copied().unwrap_or(0)
    }

    #[test]
    fn multicast_edict_uses_post_fix_non_op_return_destinations() {
        let tx = tx_with_middle_op_return();
        let rules = TransferRules::default();
        let mut unallocated = RuneSheet::new();
        let mut outputs = OutputRuneSheets::new();
        unallocated.insert(1, 500);

        rules.apply_edict(&tx, &mut unallocated, &mut outputs, 1, 300, tx.output.len() as u32);

        assert_eq!(output_amount(&outputs, 0, 1), 300);
        assert_eq!(output_amount(&outputs, 1, 1), 0);
        assert_eq!(output_amount(&outputs, 2, 1), 200);
        assert_eq!(unallocated.get(&1).copied().unwrap_or(0), 0);
    }
}
