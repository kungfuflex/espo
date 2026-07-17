use axum::response::Html;
use maud::{PreEscaped, html};

use crate::explorer::components::layout::layout_with_meta;
use crate::explorer::components::svg_assets::icon_testnet;
use crate::explorer::paths::explorer_path;

pub async fn faucet_page() -> Html<String> {
    let status_url_js = format!("{:?}", explorer_path("/api/faucet/status"));
    let send_url_js = format!("{:?}", explorer_path("/api/faucet/send"));
    let tx_prefix_js = format!("{:?}", explorer_path("/tx/"));
    let script = PreEscaped(format!(
        r#"
<script>
(() => {{
  const form = document.querySelector('[data-faucet-form]');
  if (!form) return;

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

  function number(value) {{
    const numeric = Number(value);
    return Number.isFinite(numeric)
      ? new Intl.NumberFormat(undefined, {{ maximumFractionDigits: 8 }}).format(numeric)
      : '0';
  }}

  function rpcError(data, fallback) {{
    return data && data.error && data.error.message ? data.error.message : fallback;
  }}

  function setMessage(text, tone = '') {{
    message.textContent = text;
    message.dataset.tone = tone;
  }}

  async function loadStatus() {{
    submitButton.disabled = true;
    setMessage('Loading faucet status...');
    try {{
      const response = await fetch(statusUrl, {{ headers: {{ Accept: 'application/json' }} }});
      const data = await response.json();
      if (!response.ok || data.error || !data.result) {{
        throw new Error(rpcError(data, 'Faucet status is unavailable'));
      }}
      const status = data.result;
      faucetEnabled = status.enabled === true;
      const minAmount = Number(status.min_amount);
      const maxAmount = Number(status.max_amount ?? status.amount);
      if (!Number.isFinite(minAmount) || minAmount < 0 || !Number.isFinite(maxAmount) || maxAmount < minAmount) {{
        throw new Error('Faucet amount limits are unavailable');
      }}
      amountInput.min = String(minAmount);
      amountInput.max = String(maxAmount);
      amountInput.value = String(minAmount);
      const totalAvailable = Number(status.total_available);
      available.hidden = !Number.isFinite(totalAvailable);
      availableAmount.textContent = number(totalAvailable);
      ipLimit.textContent = number(status.max_per_ip_per_day);
      limit.hidden = false;
      submitButton.disabled = !faucetEnabled;
      setMessage(faucetEnabled ? '' : 'Faucet disabled', faucetEnabled ? '' : 'warning');
    }} catch (error) {{
      faucetEnabled = false;
      submitButton.disabled = true;
      setMessage(error.message || 'Faucet status is unavailable', 'error');
    }}
  }}

  form.addEventListener('submit', async (event) => {{
    event.preventDefault();
    if (!faucetEnabled || !form.reportValidity()) return;

    const requestedAmount = Number(amountInput.value);
    submitButton.disabled = true;
    setMessage('Sending funds...');
    try {{
      const response = await fetch(sendUrl, {{
        method: 'POST',
        headers: {{ 'Content-Type': 'application/json', Accept: 'application/json' }},
        body: JSON.stringify({{ address: addressInput.value.trim(), amount: requestedAmount }})
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
      message.append(document.createTextNode(`${{number(result.amount)}} sent · `));
      const link = document.createElement('a');
      link.href = `${{txPrefix}}${{encodeURIComponent(result.txid)}}`;
      link.textContent = result.txid;
      link.className = 'mono';
      message.append(link);
    }} catch (error) {{
      setMessage(error.message || 'Faucet request failed', 'error');
    }} finally {{
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
        Some("Request regtest bitcoin from the configured B8 faucet."),
        html! {
            div class="row" {
                h1 class="h1" { "Regtest Faucet" }
            }
            p class="faucet-available" hidden data-faucet-available="" {
                "Available: "
                span class="faucet-available-icon" aria-hidden="true" { (icon_testnet()) }
                strong data-faucet-available-amount="" {}
            }
            section class="faucet-tool" {
                form class="faucet-form" data-faucet-form="" {
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
                        span class="faucet-currency" aria-hidden="true" { (icon_testnet()) }
                    }

                    p class="faucet-limit" hidden data-faucet-limit="" {
                        "Limit: "
                        span class="faucet-limit-icon" aria-hidden="true" { (icon_testnet()) }
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
