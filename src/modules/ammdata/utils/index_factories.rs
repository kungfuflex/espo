use crate::alkanes::trace::{EspoBlock, EspoSandshrewLikeTraceEvent};
use crate::modules::ammdata::storage::{AmmDataProvider, GetAmmFactoriesParams};
use crate::modules::essentials::storage::{
    EssentialsProvider, GetCreationRecordParams, GetCreationRecordsOrderedParams,
};
use crate::schemas::SchemaAlkaneId;
use anyhow::Result;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};

pub fn prepare_factories(
    block: &EspoBlock,
    provider: &AmmDataProvider,
    essentials: &EssentialsProvider,
    factories_bootstrapped: &AtomicBool,
) -> Result<(HashSet<SchemaAlkaneId>, Vec<(Vec<u8>, Vec<u8>)>)> {
    let table = provider.table();
    let mut amm_factories: HashSet<SchemaAlkaneId> = provider
        .get_amm_factories(GetAmmFactoriesParams)
        .map(|res| res.factories.into_iter().collect())
        .unwrap_or_default();
    let mut amm_factory_writes: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();

    if amm_factories.is_empty() && !factories_bootstrapped.swap(true, Ordering::Relaxed) {
        if let Ok(resp) = essentials.get_creation_records_ordered(GetCreationRecordsOrderedParams) {
            let records_len = resp.records.len();
            let mut discovered = 0usize;
            for rec in resp.records {
                if amm_factories.contains(&rec.alkane) {
                    continue;
                }
                let mut is_factory = rec
                    .inspection
                    .as_ref()
                    .map(crate::modules::ammdata::inspection_is_amm_factory)
                    .unwrap_or(false);
                if !is_factory {
                    if let Some(proxy_target) =
                        crate::modules::ammdata::lookup_proxy_target(essentials, rec.alkane)
                    {
                        if let Ok(resp) = essentials
                            .get_creation_record(GetCreationRecordParams { alkane: proxy_target })
                        {
                            if let Some(rec) = resp.record {
                                if let Some(inspection) = rec.inspection.as_ref() {
                                    if crate::modules::ammdata::inspection_is_amm_factory(
                                        inspection,
                                    ) {
                                        is_factory = true;
                                    }
                                }
                            }
                        }
                    }
                }
                if is_factory {
                    amm_factories.insert(rec.alkane);
                    amm_factory_writes.push((table.amm_factory_key(&rec.alkane), Vec::new()));
                    discovered += 1;
                }
            }
            eprintln!(
                "[AMMDATA] factory bootstrap scanned {} creation records, discovered {} factories",
                records_len, discovered
            );
        }
    }

    let mut created_alkanes: Vec<SchemaAlkaneId> = Vec::new();
    for tx in &block.transactions {
        if let Some(traces) = &tx.traces {
            for trace in traces {
                for ev in &trace.sandshrew_trace.events {
                    if let EspoSandshrewLikeTraceEvent::Create(c) = ev {
                        if let (Some(block), Some(tx)) = (
                            crate::modules::ammdata::parse_hex_u32(&c.block),
                            crate::modules::ammdata::parse_hex_u64(&c.tx),
                        ) {
                            created_alkanes.push(SchemaAlkaneId { block, tx });
                        }
                    }
                }
            }
        }
    }

    for alk in created_alkanes {
        if amm_factories.contains(&alk) {
            continue;
        }
        let mut is_factory = false;
        if let Ok(resp) = essentials.get_creation_record(GetCreationRecordParams { alkane: alk }) {
            if let Some(rec) = resp.record {
                if let Some(inspection) = rec.inspection.as_ref() {
                    if crate::modules::ammdata::inspection_is_amm_factory(inspection) {
                        is_factory = true;
                    }
                }
            }
        }
        if !is_factory {
            if let Some(proxy_target) =
                crate::modules::ammdata::lookup_proxy_target(essentials, alk)
            {
                if let Ok(resp) =
                    essentials.get_creation_record(GetCreationRecordParams { alkane: proxy_target })
                {
                    if let Some(rec) = resp.record {
                        if let Some(inspection) = rec.inspection.as_ref() {
                            if crate::modules::ammdata::inspection_is_amm_factory(inspection) {
                                is_factory = true;
                            }
                        }
                    }
                }
            }
        }
        if is_factory {
            amm_factories.insert(alk);
            amm_factory_writes.push((table.amm_factory_key(&alk), Vec::new()));
        }
    }

    Ok((amm_factories, amm_factory_writes))
}
