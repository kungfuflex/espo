use anyhow::{Result, anyhow};
use metashrew_core::index_pointer::AtomicPointer;
use protorune::message::{MessageContext, MessageContextParcel};
use protorune_support::{balance_sheet::BalanceSheet, rune_transfer::RuneTransfer};

#[derive(Clone, Default)]
pub struct AlkaneMessageContext(());

impl MessageContext for AlkaneMessageContext {
    fn protocol_tag() -> u128 {
        1
    }
    fn handle(
        _parcel: &MessageContextParcel,
    ) -> Result<(Vec<RuneTransfer>, BalanceSheet<AtomicPointer>)> {
        Err(anyhow!(
            "handle() was attempted to be called, but its disabled for Espo's AlkaneMessageContext"
        ))
    }
}
