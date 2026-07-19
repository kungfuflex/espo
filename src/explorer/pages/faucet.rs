use axum::extract::State;
use axum::response::Html;
use maud::{Markup, PreEscaped, html};

use crate::explorer::components::layout::layout_with_meta;
use crate::explorer::components::svg_assets::icon_testnet;
use crate::explorer::components::tx_view::{alkane_icon_url, icon_bg_style};
use crate::explorer::pages::state::ExplorerState;
use crate::explorer::paths::explorer_path;
use crate::schemas::SchemaAlkaneId;

fn faucet_asset_icon(class: &str, diesel_icon_style: &str) -> Markup {
    html! {
        span class=(class) aria-hidden="true" data-faucet-asset-icon="" {
            span data-faucet-icon="rbtc" { (icon_testnet()) }
            span hidden data-faucet-icon="diesel" {
                span class="alk-icon-wrap faucet-diesel-icon" {
                    span class="alk-icon-img" style=(diesel_icon_style) {}
                    span class="alk-icon-letter" { "D" }
                }
            }
        }
    }
}

pub async fn faucet_page(State(state): State<ExplorerState>) -> Html<String> {
    let diesel_id = SchemaAlkaneId { block: 2, tx: 0 };
    let diesel_icon_style = icon_bg_style(&alkane_icon_url(&diesel_id, &state.essentials_mdb));
    let status_url_js = format!("{:?}", explorer_path("/api/faucet/status"));
    let send_url_js = format!("{:?}", explorer_path("/api/faucet/send"));
    let tx_prefix_js = format!("{:?}", explorer_path("/tx/"));
    let script = PreEscaped(format!(
        r#"
<script>
(() => {{
  const form = document.querySelector('[data-faucet-form]');
  if (!form) return;

  const assetSelect = form.querySelector('[data-faucet-asset]');
  const dieselOption = assetSelect.querySelector('option[value="diesel"]');
  const addressInput = form.querySelector('[data-faucet-address]');
  const amountInput = form.querySelector('[data-faucet-amount]');
  const submitButton = form.querySelector('[data-faucet-submit]');
  const message = form.querySelector('[data-faucet-message]');
  const available = document.querySelector('[data-faucet-available]');
  const availableAmount = available.querySelector('[data-faucet-available-amount]');
  const limit = form.querySelector('[data-faucet-limit]');
  const ipLimit = form.querySelector('[data-faucet-ip-limit]');

  const statusUrl = {status_url_js};
  const sendUrl = {send_url_js};
  const txPrefix = {tx_prefix_js};
  let faucetEnabled = false;
  let faucetStatuses = {{}};

  const assetSymbols = {{ rbtc: 'rBTC', diesel: 'DIESEL' }};

  function number(value) {{
    const numeric = Number(value);
    return Number.isFinite(numeric)
      ? new Intl.NumberFormat(undefined, {{ maximumFractionDigits: 8 }}).format(numeric)
      : '0';
  }}

  function finiteNumber(value) {{
    if (value === null || value === undefined || value === '') return null;
    const numeric = Number(value);
    return Number.isFinite(numeric) ? numeric : null;
  }}

  function rpcError(data, fallback) {{
    return data && data.error && data.error.message ? data.error.message : fallback;
  }}

  function setMessage(text, tone = '') {{
    message.textContent = text;
    message.dataset.tone = tone;
  }}

  function selectedAsset() {{
    return assetSelect.value === 'diesel' ? 'diesel' : 'rbtc';
  }}

  function updateAssetIcons(asset) {{
    document.querySelectorAll('[data-faucet-icon]').forEach((icon) => {{
      icon.hidden = icon.dataset.faucetIcon !== asset;
    }});
  }}

  function applySelectedAsset(resetAmount = true) {{
    const asset = selectedAsset();
    const status = faucetStatuses[asset];
    updateAssetIcons(asset);
    if (!status) {{
      faucetEnabled = false;
      available.hidden = true;
      limit.hidden = true;
      submitButton.disabled = true;
      setMessage(`${{assetSymbols[asset]}} faucet is unavailable`, 'error');
      return;
    }}

    const minAmount = finiteNumber(status.min_amount);
    const maxAmount = finiteNumber(status.max_amount ?? status.amount);
    if (minAmount === null || minAmount < 0 || maxAmount === null || maxAmount < minAmount) {{
      throw new Error(`${{assetSymbols[asset]}} faucet amount limits are unavailable`);
    }}
    amountInput.min = String(minAmount);
    amountInput.max = String(maxAmount);
    if (resetAmount) amountInput.value = String(minAmount);

    const totalAvailable = finiteNumber(status.total_available);
    available.hidden = totalAvailable === null;
    availableAmount.textContent = totalAvailable === null ? '' : number(totalAvailable);
    const maxPerIp = finiteNumber(status.max_per_ip_per_day);
    limit.hidden = maxPerIp === null;
    ipLimit.textContent = maxPerIp === null ? '' : number(maxPerIp);

    faucetEnabled = status.enabled === true;
    submitButton.disabled = !faucetEnabled;
    setMessage(
      faucetEnabled ? '' : `${{assetSymbols[asset]}} faucet disabled`,
      faucetEnabled ? '' : 'warning'
    );
  }}

  async function loadStatus() {{
    submitButton.disabled = true;
    assetSelect.disabled = true;
    setMessage('Loading faucet status...');
    try {{
      const response = await fetch(statusUrl, {{ headers: {{ Accept: 'application/json' }} }});
      const data = await response.json();
      if (!response.ok || data.error || !data.result) {{
        throw new Error(rpcError(data, 'Faucet status is unavailable'));
      }}
      const status = data.result;
      faucetStatuses = {{
        rbtc: status.rbtc && typeof status.rbtc === 'object' ? status.rbtc : status,
        diesel: status.diesel && typeof status.diesel === 'object' ? status.diesel : null
      }};
      dieselOption.disabled = !faucetStatuses.diesel;
      dieselOption.hidden = !faucetStatuses.diesel;
      if (!faucetStatuses[selectedAsset()]) assetSelect.value = 'rbtc';
      assetSelect.disabled = false;
      applySelectedAsset(true);
    }} catch (error) {{
      faucetEnabled = false;
      submitButton.disabled = true;
      assetSelect.disabled = false;
      setMessage(error.message || 'Faucet status is unavailable', 'error');
    }}
  }}

  assetSelect.addEventListener('change', () => {{
    try {{
      applySelectedAsset(true);
    }} catch (error) {{
      faucetEnabled = false;
      submitButton.disabled = true;
      setMessage(error.message || 'Faucet status is unavailable', 'error');
    }}
  }});

  form.addEventListener('submit', async (event) => {{
    event.preventDefault();
    if (!faucetEnabled || !form.reportValidity()) return;

    const requestedAmount = Number(amountInput.value);
    const requestedAsset = selectedAsset();
    submitButton.disabled = true;
    assetSelect.disabled = true;
    setMessage('Sending funds...');
    try {{
      const response = await fetch(sendUrl, {{
        method: 'POST',
        headers: {{ 'Content-Type': 'application/json', Accept: 'application/json' }},
        body: JSON.stringify({{
          address: addressInput.value.trim(),
          amount: requestedAmount,
          asset: requestedAsset
        }})
      }});
      const data = await response.json();
      if (!response.ok || data.error || !data.result) {{
        throw new Error(rpcError(data, 'Faucet request failed'));
      }}

      const result = data.result;
      amountInput.value = number(result.amount);
      await loadStatus();
      message.textContent = '';
      message.dataset.tone = 'success';
      const resultAsset = result.asset === 'rbtc' || result.asset === 'diesel'
        ? result.asset
        : requestedAsset;
      message.append(document.createTextNode(
        `${{number(result.amount)}} ${{assetSymbols[resultAsset]}} sent · `
      ));
      const link = document.createElement('a');
      link.href = `${{txPrefix}}${{encodeURIComponent(result.txid)}}`;
      link.textContent = result.txid;
      link.className = 'mono';
      message.append(link);
    }} catch (error) {{
      setMessage(error.message || 'Faucet request failed', 'error');
    }} finally {{
      assetSelect.disabled = false;
      submitButton.disabled = !faucetEnabled;
    }}
  }});

  loadStatus();
}})();
</script>
"#,
        status_url_js = status_url_js,
        send_url_js = send_url_js,
        tx_prefix_js = tx_prefix_js,
    ));

    layout_with_meta(
        "Regtest Faucet",
        "/faucet",
        Some("Request rBTC or DIESEL from the configured B8 regtest faucet."),
        html! {
            div class="row" {
                h1 class="h1" { "Regtest Faucet" }
            }
            p class="faucet-available" hidden data-faucet-available="" {
                "Available: "
                (faucet_asset_icon("faucet-asset-icon faucet-available-icon", &diesel_icon_style))
                strong data-faucet-available-amount="" {}
            }
            section class="faucet-tool" {
                form class="faucet-form" data-faucet-form="" {
                    label class="faucet-label" for="faucet-asset" { "Asset" }
                    select
                        id="faucet-asset"
                        class="faucet-input faucet-select"
                        name="asset"
                        data-faucet-asset="" {
                        option value="rbtc" { "Regtest Bitcoin (rBTC)" }
                        option value="diesel" { "DIESEL (2:0)" }
                    }

                    label class="faucet-label" for="faucet-address" { "Regtest address" }
                    input
                        id="faucet-address"
                        class="faucet-input mono"
                        type="text"
                        name="address"
                        placeholder="bcrt1q..."
                        required
                        autocomplete="off"
                        autocapitalize="off"
                        spellcheck="false"
                        data-faucet-address="";

                    label class="faucet-label" for="faucet-amount" { "Amount per claim" }
                    div class="faucet-amount-control" {
                        input
                            id="faucet-amount"
                            class="faucet-input faucet-amount-input"
                            type="number"
                            name="amount"
                            step="0.00000001"
                            inputmode="decimal"
                            required
                            data-faucet-amount="";
                        (faucet_asset_icon("faucet-asset-icon faucet-currency", &diesel_icon_style))
                    }

                    p class="faucet-limit" hidden data-faucet-limit="" {
                        "Limit: "
                        (faucet_asset_icon("faucet-asset-icon faucet-limit-icon", &diesel_icon_style))
                        span data-faucet-ip-limit="" {}
                        " per day"
                    }

                    button class="faucet-submit" type="submit" disabled data-faucet-submit="" {
                        "Request funds"
                    }
                    p class="faucet-message" role="status" aria-live="polite" data-faucet-message="" {}
                }
            }
            (script)
        },
    )
}
