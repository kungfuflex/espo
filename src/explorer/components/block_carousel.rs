use maud::{Markup, PreEscaped, html};

use crate::explorer::mining_pools::bundled_pool_icon_svgs_json;
use crate::explorer::paths::{current_language, explorer_path};

pub fn block_carousel(current_height: Option<u64>, espo_tip: u64) -> Markup {
    let current_height = current_height.unwrap_or(espo_tip);
    let base_path_js = format!("{:?}", explorer_path("/"));
    let pool_icons_js = bundled_pool_icon_svgs_json();
    let is_chinese = current_language().is_chinese();
    let reset_label = if is_chinese { "返回最新区块" } else { "Back to latest block" };

    let script = PreEscaped(format!(
        r#"
(function() {{
  const basePath = {base_path_js};
  const isChinese = {is_chinese};
  const POOL_ICONS = {pool_icons_js};
  const basePrefix = basePath === '/' ? '' : basePath;
  const root = document.querySelector('[data-block-carousel]');
  if (!root) return;

  const scroller = root.querySelector('[data-bc-scroll]');
  const track = root.querySelector('[data-bc-track]');
  const resetButton = root.querySelector('[data-bc-reset]');
  const current = Number(root.dataset.current);
  const espoTip = Number(root.dataset.espoTip);
  if (!scroller || !track || !Number.isFinite(current) || !Number.isFinite(espoTip)) return;

  const RADIUS = 8;
  const SKELETON_BATCH = RADIUS * 2;
  const EDGE_THRESHOLD = 320;
  const RIGHT_BUFFER_VIEWPORTS = 3;
  const WINDOW_VIEWPORTS_LEFT = 2;
  const WINDOW_VIEWPORTS_RIGHT = 3;
  const MIN_WINDOW_BLOCKS = 64;
  const RETRY_MS = 1500;

  const seen = new Set();
  const blocks = [];
  let minH = current;
  let maxH = current;
  let selectedHeight = current;
  let pendingLeft = 0;
  let pendingRight = 0;
  let bufferRight = 0;
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
  let programmaticScrollRaf = null;

  let isDragging = false;
  let dragStartX = 0;
  let dragStartScrollLeft = 0;
  let lastPointerX = 0;
  let lastVelocityTs = 0;
  let velocity = 0;
  let momentumRaf = null;
  let dragMoved = false;
  let suppressClickUntil = 0;

  function cancelMomentumFrame() {{
    if (momentumRaf) {{
      cancelAnimationFrame(momentumRaf);
      momentumRaf = null;
    }}
  }}

  function stopMomentum() {{
    cancelMomentumFrame();
    velocity = 0;
  }}

  function cancelProgrammaticScroll() {{
    if (programmaticScrollRaf) {{
      cancelAnimationFrame(programmaticScrollRaf);
      programmaticScrollRaf = null;
    }}
  }}

  function stopCarouselMotion() {{
    stopMomentum();
    cancelProgrammaticScroll();
    if (scrollRaf) {{
      cancelAnimationFrame(scrollRaf);
      scrollRaf = null;
    }}
    isDragging = false;
    dragMoved = false;
    suppressClickUntil = 0;
    root.dataset.dragging = '0';
    scroller.scrollTo({{ left: scroller.scrollLeft, behavior: 'auto' }});
  }}

  function resetMomentum(x) {{
    cancelProgrammaticScroll();
    lastPointerX = x;
    lastVelocityTs = performance.now();
    velocity = 0;
    dragMoved = false;
    cancelMomentumFrame();
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
    cancelMomentumFrame();
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
    const iconSvg = POOL_ICONS[pool.slug] || POOL_ICONS.default || '';
    const icon = iconSvg ? `<span class="bc-pool-icon" aria-hidden="true">${{iconSvg}}</span>` : '';
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
    for (let i = 0; i < pendingRight + bufferRight; i++) html.push(renderSkeleton('right', i));
    track.innerHTML = html.join('');
  }}

  function slideSpan() {{
    const slide = track.querySelector('.bc-slide');
    if (!slide) return 162;
    const styles = getComputedStyle(track);
    const gap = parseFloat(styles.columnGap || styles.gap || '12') || 12;
    return slide.offsetWidth + gap;
  }}

  function rightBufferCount() {{
    return Math.max(
      SKELETON_BATCH * 2,
      Math.ceil((scroller.clientWidth * RIGHT_BUFFER_VIEWPORTS) / slideSpan())
    );
  }}

  function viewportBlockCount() {{
    return Math.max(1, Math.ceil(scroller.clientWidth / slideSpan()));
  }}

  function pruneBlocksAroundViewport() {{
    if (!blocks.length) return;
    const maxBlocks = Math.max(MIN_WINDOW_BLOCKS, viewportBlockCount() * (WINDOW_VIEWPORTS_LEFT + WINDOW_VIEWPORTS_RIGHT + 1));
    if (blocks.length <= maxBlocks) return;

    blocks.sort((a, b) => b.height - a.height);
    let selectedIndex = blocks.findIndex((block) => block.height === selectedHeight);
    if (selectedIndex < 0) {{
      selectedIndex = 0;
      let bestDist = Infinity;
      for (let i = 0; i < blocks.length; i++) {{
        const dist = Math.abs(blocks[i].height - selectedHeight);
        if (dist < bestDist) {{
          bestDist = dist;
          selectedIndex = i;
        }}
      }}
    }}

    const keepLeft = Math.max(RADIUS, viewportBlockCount() * WINDOW_VIEWPORTS_LEFT);
    const keepRight = Math.max(RADIUS, viewportBlockCount() * WINDOW_VIEWPORTS_RIGHT);
    let removeLeft = Math.max(0, selectedIndex - keepLeft);
    let removeRight = Math.max(0, blocks.length - (selectedIndex + keepRight + 1));

    if (!removeLeft && !removeRight) return;

    const span = slideSpan();
    if (removeLeft) {{
      const removed = blocks.splice(0, removeLeft);
      for (const block of removed) seen.delete(block.height);
      const delta = removeLeft * span;
      scroller.scrollLeft = Math.max(0, scroller.scrollLeft - delta);
      dragStartScrollLeft = Math.max(0, dragStartScrollLeft - delta);
    }}
    if (removeRight) {{
      const removed = blocks.splice(blocks.length - removeRight, removeRight);
      for (const block of removed) seen.delete(block.height);
    }}

    minH = blocks.reduce((min, block) => Math.min(min, block.height), blocks[0].height);
    maxH = blocks.reduce((max, block) => Math.max(max, block.height), blocks[0].height);
    leftDepleted = maxH >= espoTip;
    rightDepleted = minH <= 0;
    render();
  }}

  function realRightEdge() {{
    const realSlides = track.querySelectorAll('[data-height]');
    const lastReal = realSlides.length ? realSlides[realSlides.length - 1] : null;
    return lastReal ? lastReal.offsetLeft + lastReal.offsetWidth : 0;
  }}

  function ensureRightBuffer(count = rightBufferCount()) {{
    if (rightDepleted) return false;
    if (bufferRight >= count) return false;
    bufferRight = count;
    render();
    return true;
  }}

  function ensureRightBufferAhead() {{
    if (rightDepleted) return;
    const visibleRight = scroller.scrollLeft + scroller.clientWidth;
    const wantedRight = visibleRight + (scroller.clientWidth * RIGHT_BUFFER_VIEWPORTS);
    const needed = Math.ceil(Math.max(0, wantedRight - realRightEdge()) / slideSpan());
    ensureRightBuffer(Math.max(rightBufferCount(), needed));
  }}

  function targetLeftForHeight(height) {{
    const slide = track.querySelector(`[data-height="${{height}}"]`);
    if (!slide) return null;
    const target = slide.offsetLeft + (slide.offsetWidth / 2) - (scroller.clientWidth / 2);
    return Math.max(0, target);
  }}

  function animateScrollTo(left) {{
    cancelProgrammaticScroll();
    const start = scroller.scrollLeft;
    const delta = left - start;
    if (Math.abs(delta) < 1) {{
      scroller.scrollLeft = left;
      queueEdgeCheck();
      return;
    }}
    const duration = 420;
    let startedAt = null;
    const step = (now) => {{
      if (startedAt === null) startedAt = now;
      const elapsed = Math.min(1, (now - startedAt) / duration);
      const eased = 1 - Math.pow(1 - elapsed, 3);
      scroller.scrollLeft = start + (delta * eased);
      queueEdgeCheck();
      if (elapsed < 1) {{
        programmaticScrollRaf = requestAnimationFrame(step);
      }} else {{
        programmaticScrollRaf = null;
        updateResetButton();
      }}
    }};
    programmaticScrollRaf = requestAnimationFrame(step);
  }}

  function centerHeight(height, smooth) {{
    const target = targetLeftForHeight(height);
    if (target === null) return;
    if (smooth) {{
      animateScrollTo(target);
      return;
    }}
    cancelProgrammaticScroll();
    scroller.scrollTo({{ left: target, behavior: 'auto' }});
  }}

  async function scrollToLatest() {{
    stopCarouselMotion();
    if (!seen.has(espoTip)) {{
      const batch = await fetchWindow(espoTip);
      if (batch) {{
        applyBlocks(batch);
        render();
      }}
    }}
    stopCarouselMotion();
    requestAnimationFrame(() => centerHeight(espoTip, true));
    updateResetButton();
  }}

  function updateResetButton() {{
    if (!resetButton) return;
    root.dataset.canReset = scroller.scrollLeft > 80 ? '1' : '0';
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
    pendingRight = rightBufferCount();
    bufferRight = 0;
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
    bufferRight = rightDepleted ? 0 : rightBufferCount();
    render();
    loadingInitial = false;
    if (!initialCentered) {{
      initialCentered = true;
      requestAnimationFrame(() => centerHeight(current, false));
    }}
    updateResetButton();
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

    const hasVisualBuffer = pendingRight + bufferRight > 0;
    if (!hasVisualBuffer) {{
      pendingRight += expected;
      render();
    }}

    let loaded = false;
    let added = 0;
    try {{
      const center = Math.max(0, minH - RADIUS);
      const batch = await fetchWindow(center);
      if (!batch) {{
        scheduleRetry('right', fetchRight);
        return;
      }}
      loaded = true;
      added = batch ? applyBlocks(batch) : 0;
      if (added < expected || start === 0) rightDepleted = start === 0;
    }} finally {{
      if (loaded) {{
        pendingRight = Math.max(0, pendingRight - expected);
        bufferRight = rightDepleted ? 0 : Math.max(rightBufferCount(), bufferRight - added);
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
    pruneBlocksAroundViewport();
    updateResetButton();
    if (scroller.scrollLeft <= EDGE_THRESHOLD) fetchLeft();
    const realRemainingRight = realRightEdge() - (scroller.scrollLeft + scroller.clientWidth);
    if (realRemainingRight <= scroller.clientWidth) {{
      ensureRightBufferAhead();
    }}
    if (realRemainingRight <= Math.max(EDGE_THRESHOLD, scroller.clientWidth * 1.5)) fetchRight();
  }}

  function queueEdgeCheck() {{
    if (scrollRaf) return;
    scrollRaf = requestAnimationFrame(() => {{
      scrollRaf = null;
      checkEdges();
    }});
  }}

  scroller.addEventListener('scroll', queueEdgeCheck, {{ passive: true }});
  if (resetButton) {{
    resetButton.addEventListener('click', (event) => {{
      event.preventDefault();
      event.stopPropagation();
      scrollToLatest();
    }});
  }}

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
    queueEdgeCheck();
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
        pool_icons_js = pool_icons_js,
        is_chinese = is_chinese
    ));

    html! {
        div class="block-carousel card full-bleed" data-block-carousel data-current=(current_height) data-espo-tip=(espo_tip) data-dragging="0" {
            div class="bc-native-wrap" {
                div class="bc-native-scroll" data-bc-scroll {
                    div class="bc-native-track" data-bc-track {}
                }
                button class="bc-reset-scroll" type="button" data-bc-reset aria-label=(reset_label) title=(reset_label) {
                    svg viewBox="0 0 512 512" aria-hidden="true" focusable="false" {
                        path d="M256 512A256 256 0 1 0 256 0a256 256 0 1 0 0 512zM135 239l80-80c9.4-9.4 24.6-9.4 33.9 0s9.4 24.6 0 33.9L209.9 232H368c13.3 0 24 10.7 24 24s-10.7 24-24 24H209.9l39 39c9.4 9.4 9.4 24.6 0 33.9s-24.6 9.4-33.9 0l-80-80c-9.4-9.3-9.4-24.5 0-33.9z" {}
                    }
                }
            }
        }
        script { (script) }
    }
}
