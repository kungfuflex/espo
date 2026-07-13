use axum::response::Html;
use maud::{PreEscaped, html};

use crate::explorer::components::layout::layout_with_meta;
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
  const limits = form.querySelector('[data-faucet-limits]');
  const claims = form.querySelector('[data-faucet-claims]');
  const sent = form.querySelector('[data-faucet-sent]');
  const addressLimit = form.querySelector('[data-faucet-address-limit]');
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
      amountInput.value = number(status.amount);
      claims.textContent = number(status.claims_last_24h);
      sent.textContent = `${{number(status.sent_last_24h)}} / ${{number(status.max_per_day)}} rBTC`;
      addressLimit.textContent = `${{number(status.max_per_address_per_day)}} rBTC`;
      ipLimit.textContent = `${{number(status.max_per_ip_per_day)}} rBTC`;
      limits.hidden = false;
      submitButton.disabled = !faucetEnabled;
      setMessage(faucetEnabled ? 'Faucet available' : 'Faucet disabled', faucetEnabled ? 'success' : 'warning');
    }} catch (error) {{
      faucetEnabled = false;
      submitButton.disabled = true;
      setMessage(error.message || 'Faucet status is unavailable', 'error');
    }}
  }}

  form.addEventListener('submit', async (event) => {{
    event.preventDefault();
    if (!faucetEnabled || !form.reportValidity()) return;

    submitButton.disabled = true;
    setMessage('Sending rBTC...');
    try {{
      const response = await fetch(sendUrl, {{
        method: 'POST',
        headers: {{ 'Content-Type': 'application/json', Accept: 'application/json' }},
        body: JSON.stringify({{ address: addressInput.value.trim() }})
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
      message.append(document.createTextNode(`${{number(result.amount)}} rBTC sent · `));
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
                            type="text"
                            readonly
                            aria-readonly="true"
                            data-faucet-amount="";
                        span class="faucet-currency" { "rBTC" }
                    }

                    button class="faucet-submit" type="submit" disabled data-faucet-submit="" {
                        "Request rBTC"
                    }
                    p class="faucet-message" role="status" aria-live="polite" data-faucet-message="" {}

                    dl class="faucet-limits" hidden data-faucet-limits="" {
                        div { dt { "Claims (24h)" } dd data-faucet-claims="" {} }
                        div { dt { "Sent / daily cap" } dd data-faucet-sent="" {} }
                        div { dt { "Address cap" } dd data-faucet-address-limit="" {} }
                        div { dt { "IP cap" } dd data-faucet-ip-limit="" {} }
                    }
                }
            }
            (script)
        },
    )
}
