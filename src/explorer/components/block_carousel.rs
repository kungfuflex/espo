use maud::{Markup, PreEscaped, html};

use crate::config::get_config;
use crate::explorer::api::mempool_blocks_visible_for_espo_tip;
use crate::explorer::mining_pools::bundled_pool_icon_svgs_json;
use crate::explorer::paths::{current_language, explorer_path};
use crate::modules::runes::main::runes_enabled_from_global_config;

pub fn block_carousel(current_height: Option<u64>, espo_tip: u64) -> Markup {
    block_carousel_inner(current_height, None, espo_tip)
}

pub fn block_carousel_with_mempool(selected_mempool_index: Option<usize>, espo_tip: u64) -> Markup {
    block_carousel_inner(None, selected_mempool_index, espo_tip)
}

fn block_carousel_inner(
    current_height: Option<u64>,
    selected_mempool_index: Option<usize>,
    espo_tip: u64,
) -> Markup {
    let current_height = current_height.unwrap_or(espo_tip);
    let base_path_js = format!("{:?}", explorer_path("/"));
    let pool_icons_js = bundled_pool_icon_svgs_json();
    let mempool_cfg = &get_config().mempool;
    let ws_path = mempool_cfg.websocket_path.as_deref().unwrap_or("/api/events/ws").to_string();
    let ws_path_js = format!("{:?}", ws_path);
    let ws_enabled_js = mempool_cfg.websocket_enabled;
    let mempool_slot_count = mempool_cfg.template_blocks.max(1);
    let selected_mempool_js = selected_mempool_index
        .map(|idx| idx.to_string())
        .unwrap_or_else(|| "null".to_string());
    let is_chinese = current_language().is_chinese();
    let reset_label = if is_chinese { "返回最新区块" } else { "Back to latest block" };
    let runes_enabled = runes_enabled_from_global_config();
    let runes_enabled_js = runes_enabled;
    let mempool_blocks_enabled_js = mempool_blocks_visible_for_espo_tip(espo_tip);

    let script = PreEscaped(format!(
        r#"
(function() {{
  const basePath = {base_path_js};
  const isChinese = {is_chinese};
  const POOL_ICONS = {pool_icons_js};
  const eventsPath = {ws_path_js};
  const eventsEnabled = {ws_enabled_js};
  const MEMPOOL_SLOT_COUNT = {mempool_slot_count};
  const RUNES_ENABLED = {runes_enabled_js};
  const MEMPOOL_BLOCKS_ENABLED = {mempool_blocks_enabled_js};
  const LIVE_TIP_ANIMATION_ENABLED = MEMPOOL_BLOCKS_ENABLED;
  const basePrefix = basePath === '/' ? '' : basePath;
  const wsProtocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
  const root = document.querySelector('[data-block-carousel]');
  if (!root) return;

  const scroller = root.querySelector('[data-bc-scroll]');
  const track = root.querySelector('[data-bc-track]');
  const resetButton = root.querySelector('[data-bc-reset]');
  const current = Number(root.dataset.current);
  const selectedMempoolIndex = {selected_mempool_js};
  let selectedConfirmedHeight = selectedMempoolIndex === null ? current : null;
  let espoTip = Number(root.dataset.espoTip);
  if (!scroller || !track || !Number.isFinite(current) || !Number.isFinite(espoTip)) return;

  const RADIUS = 8;
  const SKELETON_BATCH = RADIUS * 2;
  const EDGE_THRESHOLD = 320;
  const LEFT_BUFFER_VIEWPORTS = 2;
  const RIGHT_BUFFER_VIEWPORTS = 2;
  const WINDOW_VIEWPORTS_LEFT = 1;
  const WINDOW_VIEWPORTS_RIGHT = 2;
  const MIN_VIEWPORT_BLOCKS = 10;
  const MIN_WINDOW_BLOCKS = 36;
  const RETRY_MS = 1500;
  const TIP_REFRESH_DEBOUNCE_MS = 120;
  const TIP_ANIMATION_MS = 2000;

  const seen = new Set();
  const blocks = [];
  let mempoolBlocks = [];
  let minH = current;
  let maxH = current;
  let selectedHeight = current;
  let pendingLeft = 0;
  let pendingRight = 0;
  let bufferLeft = 0;
  let bufferRight = 0;
  let loadingInitial = false;
  let loadingLeft = false;
  let loadingRight = false;
  let initialRetryTimer = null;
  let leftRetryTimer = null;
  let rightRetryTimer = null;
  let confirmAnimationUntil = 0;
  let queuedMempoolBlocks = null;
  let leftDepleted = false;
  let rightDepleted = false;
  let initialCentered = false;
  let scrollRaf = null;
  let programmaticScrollRaf = null;
  let latestTipRefreshTimer = null;
  let latestTipRefreshInFlight = false;
  let latestTipRefreshAnimate = false;
  let latestTipRefreshFromTip = null;
  let lastTipAnimationAt = 0;
  let followLatest = selectedMempoolIndex !== null || current === espoTip;
  let suppressFollowScrollUntil = 0;

  let isDragging = false;
  let dragStartX = 0;
  let dragStartScrollLeft = 0;
  let lastPointerX = 0;
  let lastVelocityTs = 0;
  let velocity = 0;
  let momentumRaf = null;
  let dragMoved = false;
  let suppressClickUntil = 0;
  let touchMomentumUntil = 0;
  let touchActive = false;

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
        queueEdgeCheck();
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
    if (RUNES_ENABLED) {{
      return isChinese ? `${{amount}} 次操作` : `${{amount}} actions`;
    }}
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
    const range = Array.isArray(block.fee_range) && block.fee_range.length
      ? block.fee_range
      : (Array.isArray(block.feeRange) ? block.feeRange : []);
    const minValue = range.length ? range[0] : block.min_fee_rate;
    const maxValue = range.length ? range[range.length - 1] : block.max_fee_rate;
    const min = formatFeeRate(minValue, false);
    const max = formatFeeRate(maxValue, true);
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

  function refreshRelativeTimes() {{
    track.querySelectorAll('[data-bc-time]').forEach((el) => {{
      const ts = Number(el.dataset.bcTime);
      if (Number.isFinite(ts) && ts > 0) {{
        el.textContent = formatAgo(ts);
      }}
    }});
  }}

  function renderBlock(block) {{
    const isShell = block.shell === true;
    const cardClass = `bc-card${{isShell ? ' bc-shell-card' : ''}}${{block.height === selectedConfirmedHeight ? ' current' : ''}}`;
    const faceMarkup = isShell
      ? '<div class="bc-shell-face" aria-hidden="true"></div>'
      : `
            ${{renderFeeStats(block)}}
            <div class="bc-traces">${{formatTraces(block.traces)}}</div>
            <div class="bc-tx-count">${{formatTxCount(block.tx_count)}}</div>
            <div class="bc-time" data-bc-time="${{block.time || ''}}">${{formatAgo(block.time)}}</div>
        `;
    return `
      <div class="bc-slide" data-height="${{block.height}}" data-bc-key="block:${{block.height}}">
        <div class="bc-top">
          <span class="bc-height-tag">${{block.height}}</span>
        </div>
        <a class="${{cardClass}}" href="${{basePrefix}}/block/${{block.height}}" draggable="false">
          <div class="bc-face">
            ${{faceMarkup}}
          </div>
          ${{block.height === selectedConfirmedHeight ? '<div class="bc-indicator" aria-hidden="true"><svg class="bc-indicator-svg" viewBox="0 0 24 14" focusable="false"><path d="M12 14L0 0h24L12 14z"></path></svg></div>' : ''}}
        </a>
        ${{renderPoolTag(block.pool)}}
      </div>
    `;
  }}

  function renderMempoolBlock(block) {{
    const href = `${{basePrefix}}/mempool-block/${{block.index + 1}}`;
    const isCurrent = Number(block.index) === selectedMempoolIndex;
    const etaMinutes = (Number(block.index) + 1) * 10;
    const etaLabel = isChinese ? `约 ${{etaMinutes}} 分钟后` : `in ~${{etaMinutes}} minutes`;
    return `
      <div class="bc-slide bc-mempool-slide" data-mempool-index="${{block.index}}" data-bc-key="mempool:${{block.index}}">
        <div class="bc-top"></div>
        <a class="bc-card bc-mempool-card${{isCurrent ? ' current' : ''}}" href="${{href}}" draggable="false">
          <div class="bc-face">
            ${{renderFeeStats(block)}}
            <div class="bc-traces">${{formatTraces(block.trace_count || 0)}}</div>
            <div class="bc-tx-count">${{formatTxCount(block.tx_count || 0)}}</div>
            <div class="bc-time">${{etaLabel}}</div>
          </div>
          ${{isCurrent ? '<div class="bc-indicator" aria-hidden="true"><svg class="bc-indicator-svg" viewBox="0 0 24 14" focusable="false"><path d="M12 14L0 0h24L12 14z"></path></svg></div>' : ''}}
        </a>
        <div class="bc-pool-slot"></div>
      </div>
    `;
  }}

  function renderMempoolSkeleton(index) {{
    return `
      <div class="bc-slide bc-mempool-slide bc-mempool-placeholder" data-mempool-index="${{index}}" data-bc-key="mempool:${{index}}">
        <div class="bc-top"></div>
        <div class="bc-card bc-card-skeleton bc-mempool-card" aria-hidden="true"></div>
        <div class="bc-pool-slot" aria-hidden="true"></div>
      </div>
    `;
  }}

  function renderBoundary() {{
    return `
      <div class="bc-boundary" data-bc-boundary data-bc-key="boundary" aria-hidden="true">
        <div class="bc-boundary-line"></div>
      </div>
    `;
  }}

  function shouldRenderMempoolSide() {{
    return MEMPOOL_BLOCKS_ENABLED && (isFollowingLatest() || maxH >= espoTip);
  }}

  function render() {{
    blocks.sort((a, b) => b.height - a.height);
    const mempoolByIndex = new Map(mempoolBlocks.map((block) => [Number(block.index), block]));
    const html = [];
    for (let i = 0; i < pendingLeft + bufferLeft; i++) html.push(renderSkeleton('left', i));
    if (shouldRenderMempoolSide()) {{
      for (let i = MEMPOOL_SLOT_COUNT - 1; i >= 0; i--) {{
        const block = mempoolByIndex.get(i);
        html.push(block ? renderMempoolBlock(block) : renderMempoolSkeleton(i));
      }}
      html.push(renderBoundary());
    }}
    for (const block of blocks) html.push(renderBlock(block));
    for (let i = 0; i < pendingRight + bufferRight; i++) html.push(renderSkeleton('right', i));
    track.innerHTML = html.join('');
    refreshRelativeTimes();
  }}

  function htmlToElement(markup) {{
    const template = document.createElement('template');
    template.innerHTML = markup.trim();
    return template.content.firstElementChild;
  }}

  function syncAttributes(target, source) {{
    Array.from(target.attributes).forEach((attr) => {{
      if (!source.hasAttribute(attr.name)) target.removeAttribute(attr.name);
    }});
    Array.from(source.attributes).forEach((attr) => {{
      if (target.getAttribute(attr.name) !== attr.value) {{
        target.setAttribute(attr.name, attr.value);
      }}
    }});
  }}

  function updateMempoolSlide(existing, next) {{
    if (!existing || !next) return false;
    syncAttributes(existing, next);
    existing.className = next.className;

    const existingTop = existing.querySelector('.bc-top');
    const nextTop = next.querySelector('.bc-top');
    if (existingTop && nextTop) existingTop.innerHTML = nextTop.innerHTML;

    const existingPool = existing.querySelector('.bc-pool-slot');
    const nextPool = next.querySelector('.bc-pool-slot');
    if (existingPool && nextPool) existingPool.innerHTML = nextPool.innerHTML;

    const existingCard = existing.querySelector('.bc-card');
    const nextCard = next.querySelector('.bc-card');
    if (!existingCard || !nextCard || existingCard.tagName !== nextCard.tagName) {{
      existing.replaceWith(next);
      return false;
    }}

    syncAttributes(existingCard, nextCard);
    existingCard.className = nextCard.className;

    const existingFace = existingCard.querySelector('.bc-face');
    const nextFace = nextCard.querySelector('.bc-face');
    if (existingFace && nextFace) {{
      existingFace.innerHTML = nextFace.innerHTML;
      refreshRelativeTimes();
    }} else {{
      existingCard.innerHTML = nextCard.innerHTML;
      refreshRelativeTimes();
      return true;
    }}

    existingCard.querySelectorAll('.bc-indicator').forEach((indicator) => indicator.remove());
    const nextIndicator = nextCard.querySelector('.bc-indicator');
    if (nextIndicator) existingCard.appendChild(nextIndicator.cloneNode(true));
    return true;
  }}

  function captureCarouselPositions() {{
    const positions = new Map();
    track.querySelectorAll('[data-bc-key]').forEach((el) => {{
      positions.set(el.dataset.bcKey, el.getBoundingClientRect());
    }});
    return positions;
  }}

  function animateCarouselFrom(previousPositions, aliases = new Map(), consumedKeys = new Set()) {{
    const animated = [];
    track.querySelectorAll('[data-bc-key]').forEach((el) => {{
      const key = el.dataset.bcKey;
      const previousKey = aliases.get(key) || key;
      const from = previousPositions.get(previousKey);
      if (!from) return;
      const to = el.getBoundingClientRect();
      const dx = from.left - to.left;
      const dy = from.top - to.top;
      if (Math.abs(dx) < 1 && Math.abs(dy) < 1) return;
      el.classList.add('bc-slide-animating');
      el.style.transition = 'none';
      el.style.transform = `translate(${{dx}}px, ${{dy}}px) translateY(-4px)`;
      animated.push(el);
    }});

    consumedKeys.forEach((key) => {{
      const el = track.querySelector(`[data-bc-key="${{key}}"]`);
      if (!el) return;
      el.classList.add('bc-slide-consumed');
      animated.push(el);
    }});

    if (!animated.length) return;
    requestAnimationFrame(() => {{
      requestAnimationFrame(() => {{
        animated.forEach((el) => {{
          el.style.transition = `transform ${{TIP_ANIMATION_MS}}ms ease, opacity ${{TIP_ANIMATION_MS}}ms ease`;
          el.style.transform = '';
          el.style.opacity = el.classList.contains('bc-slide-consumed') ? '0' : '';
        }});
      }});
    }});

    window.setTimeout(() => {{
      animated.forEach((el) => {{
        el.classList.remove('bc-slide-animating', 'bc-slide-consumed');
        el.style.transition = '';
        el.style.transform = '';
        el.style.opacity = '';
      }});
      updateResetButton();
    }}, TIP_ANIMATION_MS + 80);
  }}

  function renderPreservingAnchor(anchorSelector) {{
    const anchor = track.querySelector(anchorSelector);
    const beforeLeft = anchor ? anchor.getBoundingClientRect().left : null;
    render();
    if (beforeLeft === null) return;
    const nextAnchor = track.querySelector(anchorSelector);
    if (!nextAnchor) return;
    const afterLeft = nextAnchor.getBoundingClientRect().left;
    const delta = afterLeft - beforeLeft;
    if (Math.abs(delta) >= 1) {{
      scroller.scrollLeft += delta;
    }}
  }}

  function renderMempoolUpdate() {{
    const mempoolByIndex = new Map(mempoolBlocks.map((block) => [Number(block.index), block]));
    for (let i = MEMPOOL_SLOT_COUNT - 1; i >= 0; i--) {{
      const key = `mempool:${{i}}`;
      const existing = track.querySelector(`[data-bc-key="${{key}}"]`);
      const block = mempoolByIndex.get(i);
      const next = htmlToElement(block ? renderMempoolBlock(block) : renderMempoolSkeleton(i));
      if (!existing) {{
        const boundary = track.querySelector('[data-bc-boundary]');
        if (boundary) track.insertBefore(next, boundary);
        continue;
      }}
      updateMempoolSlide(existing, next);
    }}
    updateResetButton();
    queueEdgeCheck();
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
      SKELETON_BATCH,
      Math.ceil((scroller.clientWidth * RIGHT_BUFFER_VIEWPORTS) / slideSpan())
    );
  }}

  function leftBufferCount() {{
    return Math.max(
      SKELETON_BATCH,
      Math.ceil((scroller.clientWidth * LEFT_BUFFER_VIEWPORTS) / slideSpan())
    );
  }}

  function viewportBlockCount() {{
    return Math.max(MIN_VIEWPORT_BLOCKS, Math.ceil(scroller.clientWidth / slideSpan()));
  }}

  function isTouchMomentumActive() {{
    return performance.now() < touchMomentumUntil;
  }}

  function isUserScrollActive() {{
    return isDragging || touchActive || momentumRaf !== null || isTouchMomentumActive();
  }}

  function pruneBlocksAroundViewport() {{
    if (isUserScrollActive()) return;
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
    if (leftDepleted) bufferLeft = 0;
    render();
  }}

  function realRightEdge() {{
    const realSlides = track.querySelectorAll('[data-height]');
    const lastReal = realSlides.length ? realSlides[realSlides.length - 1] : null;
    return lastReal ? lastReal.offsetLeft + lastReal.offsetWidth : 0;
  }}

  function realLeftEdge() {{
    const realSlides = track.querySelectorAll('[data-height]');
    const firstReal = realSlides.length ? realSlides[0] : null;
    return firstReal ? firstReal.offsetLeft : 0;
  }}

  function ensureRightBuffer(count = rightBufferCount()) {{
    if (isUserScrollActive()) return false;
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

  function ensureLeftBufferAhead() {{
    if (isUserScrollActive()) return;
    if (leftDepleted || maxH >= espoTip) return;
    const desiredLeftRunway = scroller.clientWidth * LEFT_BUFFER_VIEWPORTS;
    if (scroller.scrollLeft >= desiredLeftRunway) return;
    const needed = Math.ceil((desiredLeftRunway - scroller.scrollLeft) / slideSpan());
    if (needed <= 0) return;
    bufferLeft += Math.max(SKELETON_BATCH, needed);
    withStablePrepend(() => render());
  }}

  function targetLeftForHeight(height) {{
    const slide = track.querySelector(`[data-height="${{height}}"]`);
    if (!slide) return null;
    const target = slide.offsetLeft + (slide.offsetWidth / 2) - (scroller.clientWidth / 2);
    return Math.max(0, target);
  }}

  function targetLeftForMempool(index) {{
    if (index === null || index === undefined) return null;
    const slide = track.querySelector(`[data-mempool-index="${{index}}"]`);
    if (!slide) return null;
    const target = slide.offsetLeft + (slide.offsetWidth / 2) - (scroller.clientWidth / 2);
    return Math.max(0, target);
  }}

  function targetLeftForBoundary() {{
    const boundary = track.querySelector('[data-bc-boundary]');
    if (!boundary) return targetLeftForHeight(espoTip);
    const target = boundary.offsetLeft + (boundary.offsetWidth / 2) - (scroller.clientWidth / 2);
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

  function centerMempool(index, smooth) {{
    const target = targetLeftForMempool(index);
    if (target === null) return false;
    if (smooth) {{
      animateScrollTo(target);
    }} else {{
      cancelProgrammaticScroll();
      scroller.scrollTo({{ left: target, behavior: 'auto' }});
    }}
    return true;
  }}

  function centerBoundaryOrTip(smooth) {{
    const target = targetLeftForBoundary();
    if (target !== null) {{
      if (smooth) animateScrollTo(target);
      else {{
        cancelProgrammaticScroll();
        scroller.scrollTo({{ left: target, behavior: 'auto' }});
      }}
      return true;
    }}
    centerHeight(espoTip, smooth);
    return true;
  }}

  function centerDefault(smooth) {{
    if (selectedMempoolIndex !== null && centerMempool(selectedMempoolIndex, smooth)) return;
    if (current === espoTip && centerBoundaryOrTip(smooth)) return;
    centerHeight(current, smooth);
  }}

  function restoreFollowLatest() {{
    followLatest = true;
    selectedConfirmedHeight = espoTip;
    root.dataset.current = String(espoTip);
  }}

  function markUserNavigated() {{
    if (performance.now() < suppressFollowScrollUntil) return;
    followLatest = false;
    selectedConfirmedHeight = null;
    latestTipRefreshAnimate = false;
    latestTipRefreshFromTip = null;
    if (latestTipRefreshTimer) {{
      clearTimeout(latestTipRefreshTimer);
      latestTipRefreshTimer = null;
    }}
  }}

  function isFollowingLatest() {{
    return followLatest;
  }}

  function pinLatestInView() {{
    if (!isFollowingLatest() || isDragging) return false;
    stopMomentum();
    cancelProgrammaticScroll();
    selectedHeight = espoTip;
    const target = selectedMempoolIndex !== null
      ? targetLeftForBoundary()
      : targetLeftForHeight(espoTip);
    if (target === null) return false;
    suppressFollowScrollUntil = performance.now() + 250;
    scroller.scrollTo({{ left: target, behavior: 'auto' }});
    return true;
  }}

  async function scrollToLatest() {{
    restoreFollowLatest();
    stopCarouselMotion();
    selectedHeight = espoTip;
    if (!seen.has(espoTip)) {{
      const batch = await fetchWindow(espoTip);
      if (batch) {{
        applyBlocks(batch);
        leftDepleted = maxH >= espoTip;
        if (leftDepleted) bufferLeft = 0;
        render();
      }}
    }}
    stopCarouselMotion();
    requestAnimationFrame(() => centerBoundaryOrTip(true));
    updateResetButton();
  }}

  function updateResetButton() {{
    if (!resetButton) return;
    const target = targetLeftForBoundary();
    if (target !== null) {{
      root.dataset.canReset = scroller.scrollLeft > target + 8 ? '1' : '0';
      return;
    }}
    const latest = track.querySelector(`[data-height="${{espoTip}}"]`);
    if (latest) {{
      const viewport = scroller.getBoundingClientRect();
      const rect = latest.getBoundingClientRect();
      root.dataset.canReset = rect.right < viewport.left ? '1' : '0';
    }} else {{
      root.dataset.canReset = scroller.scrollLeft > 8 ? '1' : '0';
    }}
  }}

  function withStablePrepend(renderFn) {{
    const beforeWidth = scroller.scrollWidth;
    const beforeLeft = scroller.scrollLeft;
    renderFn();
    const afterWidth = scroller.scrollWidth;
    if (afterWidth !== beforeWidth) {{
      const delta = afterWidth - beforeWidth;
      scroller.scrollLeft = Math.max(0, beforeLeft + delta);
      dragStartScrollLeft = Math.max(0, dragStartScrollLeft + delta);
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
    pendingLeft = current < espoTip ? leftBufferCount() : 0;
    pendingRight = rightBufferCount();
    bufferLeft = 0;
    bufferRight = 0;
    render();
    const batch = await fetchWindow(current);
    if (!batch) {{
      loadingInitial = false;
      scheduleRetry('initial', fetchInitial);
      return;
    }}
    applyBlocks(batch);
    leftDepleted = maxH >= espoTip;
    rightDepleted = minH <= 0;
    pendingLeft = 0;
    pendingRight = 0;
    bufferLeft = leftDepleted ? 0 : leftBufferCount();
    bufferRight = rightDepleted ? 0 : rightBufferCount();
    render();
    loadingInitial = false;
    if (!initialCentered) {{
      initialCentered = true;
      requestAnimationFrame(() => centerDefault(false));
    }}
    updateResetButton();
    queueEdgeCheck();
  }}

  async function fetchMempoolBlocks() {{
    if (!MEMPOOL_BLOCKS_ENABLED) return;
    try {{
      const res = await fetch(`${{basePrefix}}/api/mempool/blocks`, {{
        headers: {{ Accept: 'application/json' }}
      }});
      if (!res.ok) return;
      const data = await res.json();
      if (data && Array.isArray(data.blocks)) {{
        applyMempoolSnapshot(data);
      }}
    }} catch (_) {{}}
  }}

  function applyMempoolSnapshot(snapshot, force = false) {{
    if (!MEMPOOL_BLOCKS_ENABLED) return;
    if (!snapshot || !Array.isArray(snapshot.blocks)) return;
    if (!force && performance.now() < confirmAnimationUntil) {{
      queuedMempoolBlocks = snapshot.blocks;
      return;
    }}
    mempoolBlocks = snapshot.blocks;
    if (initialCentered) renderMempoolUpdate();
    else render();
  }}

  function flushQueuedMempoolBlocks() {{
    if (!MEMPOOL_BLOCKS_ENABLED) return;
    if (queuedMempoolBlocks) {{
      const next = queuedMempoolBlocks;
      queuedMempoolBlocks = null;
      applyMempoolSnapshot({{ blocks: next }}, true);
      return;
    }}
    fetchMempoolBlocks();
  }}

  function scheduleLatestTipRefresh(fromTip, shouldAnimate) {{
    if (!isFollowingLatest()) {{
      latestTipRefreshAnimate = false;
      latestTipRefreshFromTip = null;
      return;
    }}
    if (LIVE_TIP_ANIMATION_ENABLED && shouldAnimate) {{
      const now = performance.now();
      const canAnimate =
        latestTipRefreshFromTip === null &&
        espoTip === fromTip + 1 &&
        now - lastTipAnimationAt > TIP_ANIMATION_MS + 120;
      latestTipRefreshAnimate = latestTipRefreshAnimate || canAnimate;
      if (canAnimate) latestTipRefreshFromTip = fromTip;
    }}
    if (latestTipRefreshTimer || latestTipRefreshInFlight) return;
    latestTipRefreshTimer = window.setTimeout(refreshLatestTipWindow, TIP_REFRESH_DEBOUNCE_MS);
  }}

  async function refreshLatestTipWindow() {{
    latestTipRefreshTimer = null;
    if (latestTipRefreshInFlight) return;

    const targetTip = espoTip;
    const followingLatest = isFollowingLatest();
    if (!followingLatest) {{
      latestTipRefreshAnimate = false;
      latestTipRefreshFromTip = null;
      updateResetButton();
      queueEdgeCheck();
      return;
    }}

    latestTipRefreshInFlight = true;
    const shouldAnimate =
      LIVE_TIP_ANIMATION_ENABLED &&
      followingLatest &&
      latestTipRefreshAnimate &&
      latestTipRefreshFromTip !== null &&
      targetTip === latestTipRefreshFromTip + 1;
    const previousPositions = shouldAnimate ? captureCarouselPositions() : null;
    latestTipRefreshAnimate = false;
    latestTipRefreshFromTip = null;

    const batch = await fetchWindow(targetTip);
    if (!isFollowingLatest()) {{
      latestTipRefreshInFlight = false;
      updateResetButton();
      queueEdgeCheck();
      return;
    }}
    latestTipRefreshInFlight = false;
    if (!batch) {{
      scheduleLatestTipRefresh(targetTip, false);
      return;
    }}

    const added = applyBlocks(batch);
    leftDepleted = maxH >= espoTip;
    if (leftDepleted) bufferLeft = 0;
    render();
    const pinnedLatest = shouldAnimate ? false : pinLatestInView();

    if (shouldAnimate && !pinnedLatest && added === 1 && maxH === targetTip) {{
      animateCarouselFrom(
        previousPositions,
        new Map([[`block:${{targetTip}}`, 'mempool:0']]),
        new Set()
      );
      lastTipAnimationAt = performance.now();
      confirmAnimationUntil = performance.now() + TIP_ANIMATION_MS + 120;
      window.setTimeout(flushQueuedMempoolBlocks, TIP_ANIMATION_MS + 100);
    }} else {{
      flushQueuedMempoolBlocks();
    }}

    queueEdgeCheck();
    if (targetTip < espoTip) {{
      scheduleLatestTipRefresh(targetTip, false);
    }}
  }}

  function connectEvents() {{
    if (!eventsEnabled || !window.WebSocket) return;
    const wsPath = eventsPath.startsWith('/') ? `${{basePrefix}}${{eventsPath}}` : `${{basePrefix}}/${{eventsPath}}`;
    let socket;
    try {{
      socket = new WebSocket(`${{wsProtocol}}//${{window.location.host}}${{wsPath}}`);
    }} catch (_) {{
      return;
    }}
    socket.addEventListener('open', () => {{
      if (MEMPOOL_BLOCKS_ENABLED) {{
        try {{
          socket.send(JSON.stringify({{ action: 'want', data: ['mempool-blocks'] }}));
          socket.send(JSON.stringify({{ 'refresh-mempool-blocks': true }}));
        }} catch (_) {{}}
        fetchMempoolBlocks();
      }}
    }});
    socket.addEventListener('message', (event) => {{
      let payload;
      try {{
        payload = JSON.parse(event.data);
      }} catch (_) {{
        return;
      }}
      if (MEMPOOL_BLOCKS_ENABLED && payload.type === 'hello' && payload.data && payload.data.mempool) {{
        applyMempoolSnapshot(payload.data.mempool);
      }}
      if (MEMPOOL_BLOCKS_ENABLED && payload.type === 'mempool-blocks' && payload.data) {{
        applyMempoolSnapshot(payload.data);
      }}
      if (payload.type === 'block') {{
        const nextTip = Number(payload.data && payload.data.height);
        const previousTip = espoTip;
        if (Number.isFinite(nextTip) && nextTip > espoTip) {{
          const wasFollowingLatest = isFollowingLatest();
          const shouldInsertNewTip =
            wasFollowingLatest &&
            (selectedMempoolIndex !== null ||
              selectedConfirmedHeight === previousTip ||
              selectedHeight === previousTip);
          espoTip = nextTip;
          root.dataset.espoTip = String(espoTip);
          if (wasFollowingLatest && selectedConfirmedHeight === previousTip) {{
            selectedConfirmedHeight = nextTip;
            root.dataset.current = String(nextTip);
          }}
          leftDepleted = maxH >= espoTip;
          if (!shouldInsertNewTip) {{
            updateResetButton();
            queueEdgeCheck();
            return;
          }}
          scheduleLatestTipRefresh(previousTip, true);
        }}
      }}
    }});
    socket.addEventListener('close', () => window.setTimeout(connectEvents, 2500));
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

    const hasVisualBuffer = pendingLeft + bufferLeft > 0;
    if (!hasVisualBuffer && !isUserScrollActive()) {{
      pendingLeft += expected;
      withStablePrepend(() => render());
    }}

    let loaded = false;
    let added = 0;
    try {{
      const center = Math.min(espoTip, maxH + RADIUS);
      const batch = await fetchWindow(center);
      if (!batch) {{
        scheduleRetry('left', fetchLeft);
        return;
      }}
      loaded = true;
      added = batch ? applyBlocks(batch) : 0;
      if (added === 0 || added < expected || end === espoTip) leftDepleted = true;
    }} finally {{
      if (loaded) {{
        pendingLeft = Math.max(0, pendingLeft - expected);
        bufferLeft = leftDepleted ? 0 : Math.max(0, bufferLeft - added);
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
    if (!hasVisualBuffer && !isUserScrollActive()) {{
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
      if (added === 0 || added < expected || start === 0) rightDepleted = true;
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
    if (!programmaticScrollRaf) pruneBlocksAroundViewport();
    updateResetButton();
    const realRemainingLeft = realLeftEdge() - scroller.scrollLeft;
    const shouldFetchLeft = realRemainingLeft <= Math.max(EDGE_THRESHOLD, scroller.clientWidth * 1.5);
    ensureLeftBufferAhead();
    if (shouldFetchLeft) fetchLeft();
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
  scroller.addEventListener('wheel', markUserNavigated, {{ passive: true }});
  scroller.addEventListener('touchstart', () => {{
    touchActive = true;
    touchMomentumUntil = performance.now() + 1200;
    markUserNavigated();
  }}, {{ passive: true }});
  scroller.addEventListener('touchend', () => {{
    touchActive = false;
    touchMomentumUntil = performance.now() + 700;
    queueEdgeCheck();
    window.setTimeout(queueEdgeCheck, 720);
  }}, {{ passive: true }});
  scroller.addEventListener('touchcancel', () => {{
    touchActive = false;
    touchMomentumUntil = performance.now() + 700;
    queueEdgeCheck();
    window.setTimeout(queueEdgeCheck, 720);
  }}, {{ passive: true }});
  refreshRelativeTimes();
  window.setInterval(refreshRelativeTimes, 60_000);
  if (resetButton) {{
    resetButton.addEventListener('click', (event) => {{
      event.preventDefault();
      event.stopPropagation();
      scrollToLatest();
    }});
  }}

  scroller.addEventListener('mousedown', (event) => {{
    if (event.button !== 0) return;
    markUserNavigated();
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
  if (MEMPOOL_BLOCKS_ENABLED) fetchMempoolBlocks();
  connectEvents();
  fetchInitial();
}})();
"#,
        base_path_js = base_path_js,
        pool_icons_js = pool_icons_js,
        ws_path_js = ws_path_js,
        mempool_slot_count = mempool_slot_count,
        selected_mempool_js = selected_mempool_js,
        is_chinese = is_chinese,
        runes_enabled_js = runes_enabled_js,
        mempool_blocks_enabled_js = mempool_blocks_enabled_js
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
