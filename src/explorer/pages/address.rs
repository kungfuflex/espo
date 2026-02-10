use alkanes_cli_common::alkanes_pb::AlkanesTrace;
use axum::extract::{Path, Query, State};
use axum::response::Html;
use bitcoin::address::AddressType;
use bitcoin::consensus::encode::deserialize;
use bitcoin::hashes::Hash;
use bitcoin::{Address, Network, Transaction, Txid};
use bitcoincore_rpc::RpcApi;
use borsh::BorshDeserialize;
use maud::html;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::time::{Duration, Instant};

use crate::alkanes::trace::{EspoSandshrewLikeTrace, EspoTrace};
use crate::config::{get_bitcoind_rpc_client, get_electrum_like};
use crate::explorer::components::alk_balances::render_alkane_balance_cards;
use crate::explorer::components::header::{HeaderProps, HeaderSummaryItem, header, header_scripts};
use crate::explorer::components::layout::layout;
use crate::explorer::components::svg_assets::{
    icon_arrow_up_right, icon_left, icon_right, icon_skip_left, icon_skip_right,
};
use crate::explorer::components::tx_view::{TxPill, TxPillTone, render_tx};
use crate::explorer::consts::{DEFAULT_PAGE_LIMIT, MAX_PAGE_LIMIT};
use crate::explorer::pages::common::fmt_sats;
use crate::explorer::pages::state::ExplorerState;
use crate::explorer::paths::explorer_path;
use crate::modules::essentials::storage::BalanceEntry;
use crate::modules::essentials::storage::{AlkaneTxSummary, EssentialsTable};
use crate::modules::essentials::utils::balances::{
    OutpointLookup, get_balance_for_address, get_outpoint_balances_with_spent_batch,
};
use crate::runtime::mempool::{MempoolEntry, pending_by_txid, pending_for_address};
use crate::schemas::EspoOutpoint;
use crate::utils::electrum_like::{AddressHistoryEntry, ElectrumLikeBackend};

#[derive(Deserialize)]
pub struct AddressPageQuery {
    pub page: Option<usize>,
    pub limit: Option<usize>,
    pub traces: Option<String>,
    pub cursor: Option<String>,
    pub stack: Option<String>,
}

struct AddressTxRender {
    txid: Txid,
    tx: Transaction,
    traces: Option<Vec<EspoTrace>>,
    confirmations: Option<u64>,
    is_mempool: bool,
}

fn format_with_commas(n: u64) -> String {
    let mut s = n.to_string();
    let mut i = s.len() as isize - 3;
    while i > 0 {
        s.insert(i as usize, ',');
        i -= 3;
    }
    s
}

fn address_type_label(address: &Address) -> Option<&'static str> {
    match address.address_type()? {
        AddressType::P2pkh => Some("P2PKH"),
        AddressType::P2sh => Some("P2SH"),
        AddressType::P2wpkh => Some("P2WPKH"),
        AddressType::P2wsh => Some("P2WSH"),
        AddressType::P2tr => Some("P2TR"),
        _ => None,
    }
}

fn mempool_address_url(network: Network, address: &str) -> Option<String> {
    let base = match network {
        Network::Bitcoin => "https://mempool.space",
        Network::Testnet => "https://mempool.space/testnet",
        Network::Signet => "https://mempool.space/signet",
        Network::Regtest => return None,
        _ => "https://mempool.space",
    };
    Some(format!("{base}/address/{address}"))
}

fn traces_from_summary(txid: &Txid, summary: &AlkaneTxSummary) -> Vec<EspoTrace> {
    summary
        .traces
        .iter()
        .filter_map(|trace| sandshrew_to_espo_trace(txid, trace))
        .collect()
}

fn sandshrew_to_espo_trace(txid: &Txid, trace: &EspoSandshrewLikeTrace) -> Option<EspoTrace> {
    let (txid_hex, vout_s) = trace.outpoint.split_once(':')?;
    let vout = vout_s.parse::<u32>().ok()?;
    let trace_txid = Txid::from_str(txid_hex).unwrap_or(*txid);
    Some(EspoTrace {
        sandshrew_trace: trace.clone(),
        protobuf_trace: AlkanesTrace::default(),
        storage_changes: HashMap::new(),
        outpoint: EspoOutpoint { txid: trace_txid.to_byte_array().to_vec(), vout, tx_spent: None },
    })
}

pub async fn address_page(
    State(state): State<ExplorerState>,
    Path(address_raw): Path<String>,
    Query(q): Query<AddressPageQuery>,
) -> Html<String> {
    let start_time = Instant::now();
    let timeout = Duration::from_secs(10);

    let address = match Address::from_str(address_raw.trim())
        .ok()
        .and_then(|a| a.require_network(state.network).ok())
    {
        Some(a) => a,
        None => {
            return layout(
                "Address",
                html! { p class="error" { "Invalid address for this network." } },
            );
        }
    };

    let page = q.page.unwrap_or(1).max(1);
    let limit = q.limit.unwrap_or(DEFAULT_PAGE_LIMIT).clamp(1, MAX_PAGE_LIMIT);
    let traces_only = q
        .traces
        .as_deref()
        .map(|v| matches!(v, "1" | "true" | "on" | "yes"))
        .unwrap_or(true);
    let traces_param = if traces_only { "1" } else { "0" };
    let cursor_txid = q.cursor.as_ref().and_then(|v| Txid::from_str(v).ok());
    let cursor_stack: Vec<String> = q
        .stack
        .as_deref()
        .map(|v| {
            v.split(',')
                .filter_map(|s| Txid::from_str(s).ok().map(|t| t.to_string()))
                .collect()
        })
        .unwrap_or_default();

    let electrum_like = get_electrum_like();
    let address_str = address.to_string();
    let table = EssentialsTable::new(&state.essentials_mdb);
    let address_stats = electrum_like
        .address_stats(&address)
        .map_err(|e| {
            eprintln!("[address_page] failed to fetch address stats for {address_str}: {e}");
        })
        .ok();

    let balances =
        get_balance_for_address(&state.essentials_provider(), &address_str).unwrap_or_default();
    let mut balance_entries: Vec<BalanceEntry> = balances
        .into_iter()
        .map(|(alk, amt)| BalanceEntry { alkane: alk, amount: amt })
        .collect();
    balance_entries.sort_by(|a, b| {
        a.alkane.block.cmp(&b.alkane.block).then_with(|| a.alkane.tx.cmp(&b.alkane.tx))
    });

    let chain_tip = get_bitcoind_rpc_client().get_blockchain_info().ok().map(|i| i.blocks as u64);

    let off = limit.saturating_mul(page.saturating_sub(1));
    let mut pending_entries: Vec<MempoolEntry> = pending_for_address(&address_str);
    pending_entries.sort_by(|a, b| b.txid.cmp(&a.txid));
    let pending_filtered: Vec<MempoolEntry> = pending_entries
        .into_iter()
        .filter(|e| !traces_only || e.traces.as_ref().map_or(false, |t| !t.is_empty()))
        .collect();
    let pending_total = pending_filtered.len();
    let pending_set: HashSet<Txid> = pending_filtered.iter().map(|e| e.txid).collect();

    let mut tx_renders: Vec<AddressTxRender> = Vec::new();
    let mut history_error: Option<String> = None;
    let tx_has_next: bool;
    let tx_has_prev = page > 1;
    let tx_total: usize;

    let pending_slice_start = off.min(pending_total);
    let pending_slice_end = (off + limit).min(pending_total);
    for entry in pending_filtered
        .iter()
        .skip(pending_slice_start)
        .take(pending_slice_end.saturating_sub(pending_slice_start))
    {
        tx_renders.push(AddressTxRender {
            txid: entry.txid,
            tx: entry.tx.clone(),
            traces: entry.traces.clone(),
            confirmations: None,
            is_mempool: true,
        });
    }

    let remaining_slots = limit.saturating_sub(tx_renders.len());
    let confirmed_offset = off.saturating_sub(pending_total);

    let mut next_cursor: Option<Txid> = None;
    if traces_only {
        let confirmed_total = state
            .essentials_mdb
            .get(&table.alkane_address_len_key(&address_str))
            .ok()
            .flatten()
            .and_then(|b| {
                if b.len() == 8 {
                    let mut arr = [0u8; 8];
                    arr.copy_from_slice(&b);
                    Some(u64::from_le_bytes(arr) as usize)
                } else {
                    None
                }
            })
            .unwrap_or(0);
        let confirmed_slice_start = confirmed_offset.min(confirmed_total);
        let confirmed_slice_end = (confirmed_offset + remaining_slots).min(confirmed_total);

        if confirmed_slice_end > confirmed_slice_start {
            let mut txid_keys: Vec<Vec<u8>> = Vec::new();
            for idx in confirmed_slice_start..confirmed_slice_end {
                let rev_idx = confirmed_total - 1 - idx;
                txid_keys.push(table.alkane_address_txid_key(&address_str, rev_idx as u64));
            }
            let txid_vals = state.essentials_mdb.multi_get(&txid_keys).unwrap_or_default();
            let mut txids: Vec<Txid> = Vec::new();
            for v in txid_vals {
                let Some(bytes) = v else { continue };
                if bytes.len() != 32 {
                    continue;
                }
                if let Ok(txid) = Txid::from_slice(&bytes) {
                    txids.push(txid);
                }
            }

            let summary_keys: Vec<Vec<u8>> =
                txids.iter().map(|t| table.alkane_tx_summary_key(&t.to_byte_array())).collect();
            let summary_vals = state.essentials_mdb.multi_get(&summary_keys).unwrap_or_default();
            let raw_txs = electrum_like.batch_transaction_get_raw(&txids).unwrap_or_default();

            for (idx, txid) in txids.iter().enumerate() {
                let raw = raw_txs.get(idx).cloned().unwrap_or_default();
                if raw.is_empty() {
                    continue;
                }
                let tx: Transaction = match deserialize(&raw) {
                    Ok(t) => t,
                    Err(e) => {
                        eprintln!("[address_page] failed to decode tx {}: {e}", txid);
                        continue;
                    }
                };
                let summary = summary_vals
                    .get(idx)
                    .and_then(|v| v.as_ref())
                    .and_then(|b| AlkaneTxSummary::try_from_slice(b).ok());
                let confirmations = summary.as_ref().and_then(|s| {
                    let h = s.height as u64;
                    chain_tip.and_then(|tip| if tip >= h { Some(tip - h + 1) } else { None })
                });
                let traces = summary
                    .as_ref()
                    .map(|s| traces_from_summary(txid, s))
                    .filter(|t| !t.is_empty());

                tx_renders.push(AddressTxRender {
                    txid: *txid,
                    tx,
                    traces,
                    confirmations,
                    is_mempool: false,
                });
            }
        }

        tx_total = pending_total + confirmed_total;
        tx_has_next = (off + tx_renders.len()) < tx_total;
    } else {
        match address_stats.as_ref().map(|s| s.backend) {
            Some(ElectrumLikeBackend::ElectrumRpc) => {
                history_error = Some(
                    "Esplora backend is required to show all address transactions.".to_string(),
                );
                tx_total = pending_total + tx_renders.len();
                tx_has_next = false;
            }
            _ => {
                let fetch_limit = remaining_slots.max(1);
                match electrum_like.address_history_page_cursor(
                    &address,
                    cursor_txid.as_ref(),
                    fetch_limit,
                ) {
                    Ok(hist_page) => {
                        let entries: Vec<AddressHistoryEntry> = hist_page
                            .entries
                            .into_iter()
                            .filter(|e| !pending_set.contains(&e.txid))
                            .collect();
                        let confirmed_total =
                            hist_page.total.unwrap_or(entries.len()).max(entries.len());
                        let txids: Vec<Txid> =
                            entries.iter().take(remaining_slots).map(|h| h.txid).collect();
                        let summary_keys: Vec<Vec<u8>> = txids
                            .iter()
                            .map(|t| table.alkane_tx_summary_key(&t.to_byte_array()))
                            .collect();
                        let summary_vals =
                            state.essentials_mdb.multi_get(&summary_keys).unwrap_or_default();
                        let raw_txs =
                            electrum_like.batch_transaction_get_raw(&txids).unwrap_or_default();

                        for (idx, entry) in entries.iter().take(remaining_slots).enumerate() {
                            let raw = raw_txs.get(idx).cloned().unwrap_or_default();
                            if raw.is_empty() {
                                continue;
                            }
                            let tx: Transaction = match deserialize(&raw) {
                                Ok(t) => t,
                                Err(e) => {
                                    eprintln!(
                                        "[address_page] failed to decode tx {}: {e}",
                                        entry.txid
                                    );
                                    continue;
                                }
                            };
                            let summary = summary_vals
                                .get(idx)
                                .and_then(|v| v.as_ref())
                                .and_then(|b| AlkaneTxSummary::try_from_slice(b).ok());
                            let traces = summary
                                .as_ref()
                                .map(|s| traces_from_summary(&entry.txid, s))
                                .filter(|t| !t.is_empty());
                            let confirmations = entry.height.and_then(|h| {
                                chain_tip
                                    .and_then(|tip| if tip >= h { Some(tip - h + 1) } else { None })
                            });
                            tx_renders.push(AddressTxRender {
                                txid: entry.txid,
                                tx,
                                traces,
                                confirmations,
                                is_mempool: false,
                            });
                        }

                        next_cursor = entries.last().map(|e| e.txid);
                        tx_total = pending_total + confirmed_total;
                        tx_has_next = (off + tx_renders.len()) < tx_total || hist_page.has_more;
                    }
                    Err(e) => {
                        eprintln!(
                            "[address_page] failed to fetch address history for {address_str}: {e}"
                        );
                        tx_total = pending_total + tx_renders.len();
                        tx_has_next = false;
                    }
                }
            }
        }
    }

    if start_time.elapsed() > timeout {
        return layout(
            "Address",
            html! { p class="error" { "Address has too many transactions to render quickly." } },
        );
    }

    let display_start = if tx_total > 0 && off < tx_total { off + 1 } else { 0 };
    let display_end = (off + tx_renders.len()).min(tx_total);
    let last_page = if tx_total > 0 { (tx_total + limit - 1) / limit } else { 1 };
    let use_cursor = !traces_only && history_error.is_none();
    let prev_cursor = cursor_stack.last().cloned();
    let prev_stack = if cursor_stack.len() > 1 {
        cursor_stack[..cursor_stack.len() - 1].join(",")
    } else {
        String::new()
    };
    let current_cursor = cursor_txid.map(|t| t.to_string());
    let mut next_stack_vec = cursor_stack.clone();
    if let Some(cur) = current_cursor {
        next_stack_vec.push(cur);
    }
    let next_stack = if next_stack_vec.is_empty() { None } else { Some(next_stack_vec.join(",")) };
    let next_cursor_str = next_cursor.map(|t| t.to_string());
    let base_path = explorer_path(&format!("/address/{}", address_str));
    let first_href = format!("{base_path}?page=1&limit={limit}&traces={traces_param}");
    let prev_href = if use_cursor {
        let mut q = format!("page={}&limit={limit}&traces={traces_param}", page - 1);
        if let Some(prev) = prev_cursor.as_ref() {
            q.push_str(&format!("&cursor={}", prev));
        }
        if !prev_stack.is_empty() {
            q.push_str(&format!("&stack={}", prev_stack));
        }
        format!("{base_path}?{q}")
    } else {
        format!("{base_path}?page={}&limit={limit}&traces={traces_param}", page - 1)
    };
    let next_href = if use_cursor {
        let mut q = format!("page={}&limit={limit}&traces={traces_param}", page + 1);
        if let Some(next) = next_cursor_str.as_ref() {
            q.push_str(&format!("&cursor={}", next));
        }
        if let Some(stack) = next_stack.as_ref() {
            q.push_str(&format!("&stack={}", stack));
        }
        format!("{base_path}?{q}")
    } else {
        format!("{base_path}?page={}&limit={limit}&traces={traces_param}", page + 1)
    };
    let last_href = format!("{base_path}?page={last_page}&limit={limit}&traces={traces_param}");

    let mut prev_txids: Vec<Txid> = Vec::new();
    for item in &tx_renders {
        for vin in &item.tx.input {
            if !vin.previous_output.is_null() {
                prev_txids.push(vin.previous_output.txid);
            }
        }
    }

    prev_txids.sort();
    prev_txids.dedup();
    let mut prev_map: HashMap<Txid, Transaction> = HashMap::new();
    if !prev_txids.is_empty() {
        let raw_prev = electrum_like.batch_transaction_get_raw(&prev_txids).unwrap_or_default();
        for (i, raw) in raw_prev.into_iter().enumerate() {
            if raw.is_empty() {
                if let Some(mempool_prev) = pending_by_txid(&prev_txids[i]) {
                    prev_map.insert(prev_txids[i], mempool_prev.tx);
                }
                continue;
            }
            if let Ok(prev_tx) = deserialize::<Transaction>(&raw) {
                prev_map.insert(prev_txids[i], prev_tx);
            } else if let Some(mempool_prev) = pending_by_txid(&prev_txids[i]) {
                prev_map.insert(prev_txids[i], mempool_prev.tx);
            }
        }
    }

    let mut all_outpoints: Vec<(Txid, u32)> = Vec::new();
    for item in &tx_renders {
        for (vout, _) in item.tx.output.iter().enumerate() {
            all_outpoints.push((item.txid, vout as u32));
        }
        for vin in &item.tx.input {
            if !vin.previous_output.is_null() {
                all_outpoints.push((vin.previous_output.txid, vin.previous_output.vout));
            }
        }
    }
    all_outpoints.sort();
    all_outpoints.dedup();
    let outpoint_map =
        get_outpoint_balances_with_spent_batch(&state.essentials_provider(), &all_outpoints)
            .unwrap_or_default();
    let outpoint_fn = move |txid: &Txid, vout: u32| -> OutpointLookup {
        outpoint_map.get(&(*txid, vout)).cloned().unwrap_or_default()
    };
    let outspends_map: std::collections::HashMap<Txid, Vec<Option<Txid>>> = {
        let mut dedup = tx_renders.iter().map(|t| t.txid).collect::<Vec<_>>();
        dedup.sort();
        dedup.dedup();
        let fetched = electrum_like.batch_transaction_get_outspends(&dedup).unwrap_or_default();
        dedup.into_iter().zip(fetched.into_iter()).collect()
    };
    let outspends_fn = move |txid: &Txid| -> Vec<Option<Txid>> {
        outspends_map.get(txid).cloned().unwrap_or_default()
    };

    let balances_markup = if balance_entries.is_empty() {
        html! { p class="muted" { "No alkanes tracked for this address." } }
    } else {
        render_alkane_balance_cards(&balance_entries, &state.essentials_mdb)
    };

    let mut summary_items: Vec<HeaderSummaryItem> = Vec::new();
    summary_items.push(HeaderSummaryItem {
        label: "Confirmed balance".to_string(),
        value: match address_stats.as_ref().and_then(|s| s.confirmed_balance) {
            Some(sats) => html! { span class="summary-value" { (fmt_sats(sats)) } },
            None => html! { span class="summary-value muted" { "—" } },
        },
    });
    summary_items.push(HeaderSummaryItem {
        label: "Total Received".to_string(),
        value: match (
            address_stats.as_ref().and_then(|s| s.total_received),
            address_stats.as_ref().map(|s| s.backend),
        ) {
            (Some(total), _) => html! { span class="summary-value" { (fmt_sats(total)) } },
            (None, Some(ElectrumLikeBackend::ElectrumRpc)) => {
                html! { span class="summary-value muted" { "Unsupported" } }
            }
            _ => html! { span class="summary-value muted" { "Unavailable" } },
        },
    });
    summary_items.push(HeaderSummaryItem {
        label: "Confirmed UTXOs".to_string(),
        value: match address_stats.as_ref().and_then(|s| s.confirmed_utxos) {
            Some(count) => {
                html! { span class="summary-value" { (format_with_commas(count as u64)) } }
            }
            None => html! { span class="summary-value muted" { "—" } },
        },
    });
    summary_items.push(HeaderSummaryItem {
        label: "Address Type".to_string(),
        value: match address_type_label(&address) {
            Some(t) => html! { span class="summary-value" { span class="pill small" { (t) } } },
            None => html! { span class="summary-value muted" { "Unknown" } },
        },
    });

    let header_markup = header(HeaderProps {
        title: "Address".to_string(),
        id: Some(address_str.clone()),
        show_copy: true,
        pill: None,
        summary_items,
        cta: None,
        hero_class: Some("tx-hero-address".to_string()),
    });

    let mempool_url = mempool_address_url(state.network, &address_str);

    layout(
        &format!("Address {address_str}"),
        html! {
            (header_markup)
            @if let Some(url) = mempool_url {
                div class="tx-mempool-row" {
                    a class="tx-mempool-link" href=(url) target="_blank" rel="noopener noreferrer" {
                        "view on mempool.space"
                        (icon_arrow_up_right())
                    }
                }
            }

            h2 class="h2" { "Alkane Balances" }
            (balances_markup)

            div class="card" {
                div class="row" {
                    h2 class="h2" { "Transactions" }
                    form class="trace-toggle" method="get" action=(explorer_path(&format!("/address/{}", address_str))) {
                        input type="hidden" name="page" value="1";
                        input type="hidden" name="limit" value=(limit);
                        input type="hidden" name="traces" value=(traces_param);
                        label class="switch" {
                            span class="switch-label" { "Only Alkanes txs" }
                            input
                                class="switch-input"
                                type="checkbox"
                                checked[traces_only]
                                onchange="this.form.traces.value = this.checked ? '1' : '0'; this.form.submit();";
                            span class="switch-slider" {}
                        }
                    }
                }

                @if let Some(err) = history_error.as_ref() {
                    p class="error" { (err) }
                }
                @if tx_total == 0 {
                    p class="muted" { "No transactions found." }
                } @else {
                    div class="list" {
                        @for item in tx_renders {
                            @let traces_ref: Option<&[EspoTrace]> = item.traces.as_ref().map(|v| v.as_slice());
                            @let pill = if item.is_mempool {
                                Some(TxPill { label: "Unconfirmed".to_string(), tone: TxPillTone::Danger })
                            } else if let Some(c) = item.confirmations {
                                Some(TxPill {
                                    label: format!("{} confirmations", format_with_commas(c)),
                                    tone: TxPillTone::Success,
                                })
                            } else {
                                None
                            };
                            (render_tx(&item.txid, &item.tx, traces_ref, state.network, &prev_map, &outpoint_fn, &outspends_fn, &state.essentials_mdb, pill, true))
                        }
                    }

                    div class="pager" {
                        @if tx_has_prev {
                            a class="pill iconbtn" href=(first_href) aria-label="First page" {
                                (icon_skip_left())
                            }
                        } @else {
                            span class="pill disabled iconbtn" aria-hidden="true" { (icon_skip_left()) }
                        }
                        @if tx_has_prev {
                            a class="pill iconbtn" href=(prev_href) aria-label="Previous page" {
                                (icon_left())
                            }
                        } @else {
                            span class="pill disabled iconbtn" aria-hidden="true" { (icon_left()) }
                        }
                        span class="pager-meta muted" { "Showing "
                            (if tx_total > 0 { display_start } else { 0 })
                            @if tx_total > 0 {
                                "-"
                                (display_end)
                            }
                            " / "
                            (tx_total)
                        }
                        @if tx_has_next {
                            a class="pill iconbtn" href=(next_href) aria-label="Next page" {
                                (icon_right())
                            }
                        } @else {
                            span class="pill disabled iconbtn" aria-hidden="true" { (icon_right()) }
                        }
                        @if tx_has_next && !use_cursor {
                            a class="pill iconbtn" href=(last_href) aria-label="Last page" {
                                (icon_skip_right())
                            }
                        } @else {
                            span class="pill disabled iconbtn" aria-hidden="true" { (icon_skip_right()) }
                        }
                    }
                }
            }

            (header_scripts())
        },
    )
}
