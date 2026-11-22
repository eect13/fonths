/**
 * Virtual scroll window math that scales safely past Chromium's ~16.7M px
 * CSS element height ceiling so 100,000+ font libraries scroll smoothly from
 * item 1 all the way to the very last item.
 */
export const MAX_VIRTUAL_SCROLL_HEIGHT = 12_000_000;

export interface VirtualWindowInput {
  totalItems: number;
  cols: number;
  rowH: number;
  gap: number;
  scrollTop: number;
  viewportH: number;
  overscan?: number;
  paddingBottom?: number;
  maxScrollH?: number;
}

export interface VirtualWindowResult {
  rows: number;
  rawTotalH: number;
  totalH: number;
  startRow: number;
  endRow: number;
  start: number;
  end: number;
  offsetY: number;
  scaled: boolean;
}

export function computeVirtualWindow({
  totalItems,
  cols,
  rowH,
  gap,
  scrollTop,
  viewportH,
  overscan = 2,
  paddingBottom = 24,
  maxScrollH = MAX_VIRTUAL_SCROLL_HEIGHT,
}: VirtualWindowInput): VirtualWindowResult {
  const safeCols = Math.max(1, cols);
  const stride = Math.max(1, rowH + gap);
  if (totalItems <= 0) {
    return {
      rows: 0,
      rawTotalH: 0,
      totalH: 0,
      startRow: 0,
      endRow: 0,
      start: 0,
      end: 0,
      offsetY: 0,
      scaled: false,
    };
  }
  const rows = Math.max(1, Math.ceil(totalItems / safeCols));
  const rawTotalH = rows * rowH + Math.max(0, rows - 1) * gap + paddingBottom;
  const safeViewportH = Math.max(1, viewportH);

  if (rawTotalH <= maxScrollH) {
    const startRow = Math.max(0, Math.floor(scrollTop / stride) - overscan);
    const endRow = Math.min(rows, Math.ceil((scrollTop + safeViewportH) / stride) + overscan);
    return {
      rows,
      rawTotalH,
      totalH: rawTotalH,
      startRow,
      endRow,
      start: startRow * safeCols,
      end: Math.min(totalItems, endRow * safeCols),
      offsetY: startRow * stride,
      scaled: false,
    };
  }

  const totalH = maxScrollH;
  const maxDomScroll = Math.max(1, totalH - safeViewportH);
  const maxRawScroll = Math.max(1, rawTotalH - safeViewportH);
  const ratio = Math.min(1, Math.max(0, scrollTop / maxDomScroll));
  const virtualScrollTop = ratio * maxRawScroll;
  const startRow = Math.max(0, Math.floor(virtualScrollTop / stride) - overscan);
  const endRow = Math.min(rows, Math.ceil((virtualScrollTop + safeViewportH) / stride) + overscan);
  const subRowDelta = virtualScrollTop - startRow * stride;
  const offsetY = Math.max(0, Math.min(totalH, scrollTop - subRowDelta));
  return {
    rows,
    rawTotalH,
    totalH,
    startRow,
    endRow,
    start: startRow * safeCols,
    end: Math.min(totalItems, endRow * safeCols),
    offsetY,
    scaled: true,
  };
}
