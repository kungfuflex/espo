use maud::{Markup, PreEscaped, html};

use crate::explorer::paths::{current_language, explorer_path};

pub fn block_carousel(current_height: Option<u64>, espo_tip: u64) -> Markup {
    let current_height = current_height.unwrap_or(espo_tip);
    let base_path_js = format!("{:?}", explorer_path("/"));
    let is_chinese = current_language().is_chinese();

    let script = PreEscaped(format!(
        r#"
(function() {{
  const basePath = {base_path_js};
  const isChinese = {is_chinese};
  const basePrefix = basePath === '/' ? '' : basePath;
  const root = document.querySelector('[data-block-carousel]');
  if (!root) return;

  const scroller = root.querySelector('[data-bc-scroll]');
  const track = root.querySelector('[data-bc-track]');
  const current = Number(root.dataset.current);
  const espoTip = Number(root.dataset.espoTip);
  if (!scroller || !track || !Number.isFinite(current) || !Number.isFinite(espoTip)) return;

  const RADIUS = 8;
  const SKELETON_BATCH = RADIUS * 2;
  const EDGE_THRESHOLD = 320;
  const RETRY_MS = 1500;

  const seen = new Set();
  const blocks = [];
  let minH = current;
  let maxH = current;
  let selectedHeight = current;
  let pendingLeft = 0;
  let pendingRight = 0;
  let loadingInitial = false;
  let loadingLeft = false;
  let loadingRight = false;
  let initialRetryTimer = null;
  let leftRetryTimer = null;
  let rightRetryTimer = null;
  let leftDepleted = false;
  let rightDepleted = false;
  let initialCentered = false;
  let scrollRaf = null;

  let isDragging = false;
  let dragStartX = 0;
  let dragStartScrollLeft = 0;
  let lastPointerX = 0;
  let lastVelocityTs = 0;
  let velocity = 0;
  let momentumRaf = null;
  let dragMoved = false;
  let suppressClickUntil = 0;

  function stopMomentum() {{
    if (momentumRaf) {{
      cancelAnimationFrame(momentumRaf);
      momentumRaf = null;
    }}
  }}

  function resetMomentum(x) {{
    lastPointerX = x;
    lastVelocityTs = performance.now();
    velocity = 0;
    dragMoved = false;
    stopMomentum();
  }}

  function updateVelocity(x) {{
    const now = performance.now();
    const dt = now - lastVelocityTs;
    if (dt <= 0) return;
    const inst = (x - lastPointerX) / dt;
    velocity = (velocity * 0.8) + (inst * 0.2);
    lastPointerX = x;
    lastVelocityTs = now;
  }}

  function animateMomentum() {{
    stopMomentum();
    if (Math.abs(velocity) < 0.01) return;
    let last = performance.now();
    const step = (now) => {{
      const dt = now - last;
      last = now;
      if (Math.abs(velocity) < 0.01) {{
        momentumRaf = null;
        return;
      }}
      scroller.scrollLeft -= velocity * dt;
      velocity *= Math.pow(0.94, dt / 16);
      momentumRaf = requestAnimationFrame(step);
    }};
    momentumRaf = requestAnimationFrame(step);
  }}

  function formatAgo(ts) {{
    if (!ts) return '';
    const diff = Math.max(0, Date.now() / 1000 - ts);
    const mins = Math.floor(diff / 60);
    const hrs = Math.floor(mins / 60);
    const days = Math.floor(hrs / 24);
    if (isChinese) {{
      if (days > 365) return `${{Math.floor(days / 365)}}年前`;
      if (days > 30) return `${{Math.floor(days / 30)}}个月前`;
      if (days > 0) return `${{days}}天前`;
      if (hrs > 0) return `${{hrs}}小时前`;
      if (mins > 0) return `${{mins}}分钟前`;
      return '刚刚';
    }}
    if (days > 365) return `${{Math.floor(days / 365)}}y ago`;
    if (days > 30) return `${{Math.floor(days / 30)}}mo ago`;
    if (days > 0) return `${{days}}d ago`;
    if (hrs > 0) return `${{hrs}}h ago`;
    if (mins > 0) return `${{mins}}m ago`;
    return 'just now';
  }}

  function formatTraces(count) {{
    const amount = Number.isFinite(Number(count))
      ? new Intl.NumberFormat('en-US', {{ maximumFractionDigits: 0 }}).format(Number(count))
      : String(count);
    return isChinese ? `${{amount}} 条跟踪` : `${{amount}} traces`;
  }}

  function formatTxCount(count) {{
    if (!Number.isFinite(Number(count))) return '';
    const amount = new Intl.NumberFormat('en-US', {{ maximumFractionDigits: 0 }}).format(Number(count));
    return isChinese ? `${{amount}} 笔交易` : `${{amount}} transactions`;
  }}

  function formatFeeRate(rate, includeUnit = true) {{
    const numeric = Number(rate);
    if (!Number.isFinite(numeric)) return '';
    const compact = Math.abs(numeric) >= 1000;
    const displayValue = compact ? numeric / 1000 : numeric;
    const amount = new Intl.NumberFormat('en-US', {{
      minimumFractionDigits: 0,
      maximumFractionDigits: 2
    }}).format(displayValue) + (compact ? 'k' : '');
    if (!includeUnit) return amount;
    return `${{amount}} sat/vB`;
  }}

  function renderFeeStats(block) {{
    const median = formatFeeRate(block.median_fee_rate, true);
    const min = formatFeeRate(block.min_fee_rate, false);
    const max = formatFeeRate(block.max_fee_rate, true);
    if (!median && !min && !max) return '';
    const medianMarkup = median ? `<div class="bc-fees">~${{median}}</div>` : '';
    const rangeMarkup = min && max ? `<div class="bc-fee-span">${{min}} - ${{max}}</div>` : '';
    return `${{medianMarkup}}${{rangeMarkup}}`;
  }}

  function escapeHtml(value) {{
    return String(value ?? '')
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;')
      .replace(/'/g, '&#39;');
  }}

  function renderPoolTag(pool) {{
    if (!pool || !pool.name) return '<div class="bc-pool-slot"></div>';
    const icon = pool.icon_url
      ? `<img class="bc-pool-icon" src="${{escapeHtml(pool.icon_url)}}" alt="${{escapeHtml(pool.name)}} pool icon" loading="lazy">`
      : '';
    const unknownClass = pool.matched ? '' : ' unknown';
    const content = `
      ${{icon}}
      <span class="bc-pool-name">${{escapeHtml(pool.name)}}</span>
    `;
    if (pool.mempool_url) {{
      return `<a class="bc-pool-tag${{unknownClass}}" href="${{escapeHtml(pool.mempool_url)}}" target="_blank" rel="noopener noreferrer">${{content}}</a>`;
    }}
    return `<div class="bc-pool-tag${{unknownClass}}">${{content}}</div>`;
  }}

  function renderSkeleton(side, index) {{
    return `
      <div class="bc-slide bc-skeleton" data-bc-skeleton="${{side}}-${{index}}">
        <div class="bc-top" aria-hidden="true"></div>
        <div class="bc-card bc-card-skeleton" aria-hidden="true"></div>
        <div class="bc-pool-slot" aria-hidden="true"></div>
      </div>
    `;
  }}

  function renderBlock(block) {{
    return `
      <div class="bc-slide" data-height="${{block.height}}">
        <div class="bc-top">
          <span class="bc-height-tag">${{block.height}}</span>
        </div>
        <a class="bc-card${{block.height === current ? ' current' : ''}}" href="${{basePrefix}}/block/${{block.height}}" draggable="false">
          <div class="bc-face">
            ${{renderFeeStats(block)}}
            <div class="bc-traces">${{formatTraces(block.traces)}}</div>
            <div class="bc-tx-count">${{formatTxCount(block.tx_count)}}</div>
            <div class="bc-time">${{formatAgo(block.time)}}</div>
          </div>
          ${{block.height === current ? '<div class="bc-indicator" aria-hidden="true"><svg class="bc-indicator-svg" viewBox="0 0 24 14" focusable="false"><path d="M12 14L0 0h24L12 14z"></path></svg></div>' : ''}}
        </a>
        ${{renderPoolTag(block.pool)}}
      </div>
    `;
  }}

  function render() {{
    blocks.sort((a, b) => b.height - a.height);
    const html = [];
    for (let i = 0; i < pendingLeft; i++) html.push(renderSkeleton('left', i));
    for (const block of blocks) html.push(renderBlock(block));
    for (let i = 0; i < pendingRight; i++) html.push(renderSkeleton('right', i));
    track.innerHTML = html.join('');
  }}

  function centerHeight(height, smooth) {{
    const slide = track.querySelector(`[data-height="${{height}}"]`);
    if (!slide) return;
    const target = slide.offsetLeft + (slide.offsetWidth / 2) - (scroller.clientWidth / 2);
    scroller.scrollTo({{ left: Math.max(0, target), behavior: smooth ? 'smooth' : 'auto' }});
  }}

  function withStablePrepend(renderFn) {{
    const beforeWidth = scroller.scrollWidth;
    const beforeLeft = scroller.scrollLeft;
    renderFn();
    const afterWidth = scroller.scrollWidth;
    if (afterWidth !== beforeWidth) {{
      scroller.scrollLeft = Math.max(0, beforeLeft + (afterWidth - beforeWidth));
    }}
  }}

  function scheduleRetry(side, fn) {{
    const existing = side === 'initial'
      ? initialRetryTimer
      : side === 'left'
        ? leftRetryTimer
        : rightRetryTimer;
    if (existing) return;

    const timer = setTimeout(() => {{
      if (side === 'initial') initialRetryTimer = null;
      if (side === 'left') leftRetryTimer = null;
      if (side === 'right') rightRetryTimer = null;
      fn();
    }}, RETRY_MS);

    if (side === 'initial') initialRetryTimer = timer;
    if (side === 'left') leftRetryTimer = timer;
    if (side === 'right') rightRetryTimer = timer;
  }}

  function applyBlocks(batch) {{
    let added = 0;
    for (const block of batch) {{
      if (block.height > espoTip) continue;
      if (seen.has(block.height)) continue;
      seen.add(block.height);
      blocks.push(block);
      minH = Math.min(minH, block.height);
      maxH = Math.max(maxH, block.height);
      added += 1;
    }}
    return added;
  }}

  async function fetchWindow(center) {{
    try {{
      const res = await fetch(`${{basePrefix}}/api/blocks/carousel?center=${{center}}&radius=${{RADIUS}}`, {{
        headers: {{ Accept: 'application/json' }}
      }});
      if (!res.ok) return null;
      const data = await res.json();
      return data && Array.isArray(data.blocks) ? data.blocks : null;
    }} catch (_) {{
      return null;
    }}
  }}

  async function fetchInitial() {{
    if (loadingInitial || initialRetryTimer) return;
    loadingInitial = true;
    pendingLeft = current < espoTip ? RADIUS : 0;
    pendingRight = RADIUS + 1;
    render();
    const batch = await fetchWindow(current);
    if (!batch) {{
      loadingInitial = false;
      scheduleRetry('initial', fetchInitial);
      return;
    }}
    applyBlocks(batch);
    pendingLeft = 0;
    pendingRight = 0;
    render();
    loadingInitial = false;
    if (!initialCentered) {{
      initialCentered = true;
      requestAnimationFrame(() => centerHeight(current, false));
    }}
    queueEdgeCheck();
  }}

  async function fetchLeft() {{
    if (loadingLeft || leftRetryTimer || leftDepleted || maxH >= espoTip) return;
    loadingLeft = true;
    const end = Math.min(espoTip, maxH + SKELETON_BATCH);
    const expected = end - maxH;
    if (expected <= 0) {{
      leftDepleted = true;
      loadingLeft = false;
      return;
    }}

    const hasPending = pendingLeft > 0;
    if (!hasPending) {{
      pendingLeft += expected;
      withStablePrepend(() => render());
    }}

    let loaded = false;
    try {{
      const center = Math.min(espoTip, maxH + RADIUS);
      const batch = await fetchWindow(center);
      if (!batch) {{
        scheduleRetry('left', fetchLeft);
        return;
      }}
      loaded = true;
      const added = batch ? applyBlocks(batch) : 0;
      if (added < expected || end === espoTip) leftDepleted = end === espoTip;
    }} finally {{
      if (loaded) {{
        pendingLeft = Math.max(0, pendingLeft - expected);
        withStablePrepend(() => render());
      }}
      loadingLeft = false;
      if (loaded) queueEdgeCheck();
    }}
  }}

  async function fetchRight() {{
    if (loadingRight || rightRetryTimer || rightDepleted || minH <= 0) return;
    loadingRight = true;
    const start = Math.max(0, minH - SKELETON_BATCH);
    const expected = minH - start;
    if (expected <= 0) {{
      rightDepleted = true;
      loadingRight = false;
      return;
    }}

    const hasPending = pendingRight > 0;
    if (!hasPending) {{
      pendingRight += expected;
      render();
    }}

    let loaded = false;
    try {{
      const center = Math.max(0, minH - RADIUS);
      const batch = await fetchWindow(center);
      if (!batch) {{
        scheduleRetry('right', fetchRight);
        return;
      }}
      loaded = true;
      const added = batch ? applyBlocks(batch) : 0;
      if (added < expected || start === 0) rightDepleted = start === 0;
    }} finally {{
      if (loaded) {{
        pendingRight = Math.max(0, pendingRight - expected);
        render();
      }}
      loadingRight = false;
      if (loaded) queueEdgeCheck();
    }}
  }}

  function updateSelectedHeight() {{
    const slides = Array.from(track.querySelectorAll('[data-height]'));
    if (!slides.length) return;
    const viewportRect = scroller.getBoundingClientRect();
    const viewportCenter = viewportRect.left + (viewportRect.width / 2);
    let bestHeight = selectedHeight;
    let bestDist = Infinity;
    for (const slide of slides) {{
      const rect = slide.getBoundingClientRect();
      const center = rect.left + (rect.width / 2);
      const dist = Math.abs(center - viewportCenter);
      if (dist < bestDist) {{
        bestDist = dist;
        bestHeight = Number(slide.dataset.height);
      }}
    }}
    selectedHeight = bestHeight;
  }}

  function checkEdges() {{
    updateSelectedHeight();
    if (scroller.scrollLeft <= EDGE_THRESHOLD) fetchLeft();
    if (scroller.scrollLeft + scroller.clientWidth >= scroller.scrollWidth - EDGE_THRESHOLD) fetchRight();
  }}

  function queueEdgeCheck() {{
    if (scrollRaf) return;
    scrollRaf = requestAnimationFrame(() => {{
      scrollRaf = null;
      checkEdges();
    }});
  }}

  scroller.addEventListener('scroll', queueEdgeCheck, {{ passive: true }});

  scroller.addEventListener('mousedown', (event) => {{
    if (event.button !== 0) return;
    isDragging = true;
    dragStartX = event.clientX;
    dragStartScrollLeft = scroller.scrollLeft;
    resetMomentum(event.clientX);
    root.dataset.dragging = '1';
    event.preventDefault();
  }});

  document.addEventListener('mousemove', (event) => {{
    if (!isDragging) return;
    if (Math.abs(event.clientX - dragStartX) > 4) dragMoved = true;
    updateVelocity(event.clientX);
    scroller.scrollLeft = dragStartScrollLeft - (event.clientX - dragStartX);
  }});

  document.addEventListener('mouseup', () => {{
    if (!isDragging) return;
    isDragging = false;
    root.dataset.dragging = '0';
    if (dragMoved) suppressClickUntil = performance.now() + 450;
    animateMomentum();
  }});

  scroller.addEventListener('dragstart', (event) => event.preventDefault());
  scroller.addEventListener('click', (event) => {{
    if (dragMoved || performance.now() < suppressClickUntil) {{
      event.preventDefault();
      event.stopPropagation();
      event.stopImmediatePropagation();
    }}
    dragMoved = false;
  }}, true);

  render();
  fetchInitial();
}})();
"#,
        base_path_js = base_path_js,
        is_chinese = is_chinese
    ));

    html! {
        div class="block-carousel card full-bleed" data-block-carousel data-current=(current_height) data-espo-tip=(espo_tip) data-dragging="0" {
            div class="bc-native-wrap" {
                div class="bc-native-scroll" data-bc-scroll {
                    div class="bc-native-track" data-bc-track {}
                }
            }
        }
        script { (script) }
    }
}
