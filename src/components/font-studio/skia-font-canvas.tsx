import { memo, useEffect, useMemo, useRef, useState } from "react";
import {
  cssColorToHex,
  renderFontSpecimen,
  SKIA_SUPERSEDED,
  type RenderFontSpecimenOptions,
} from "@/lib/fonts/skia-engine";
import { cn } from "@/lib/utils";

export interface SkiaFontCanvasProps {
  /** Per-canvas coalescing slot (newer renders replace queued older ones for the same slot). */
  slot?: string;
  /** All on-disk files for the family so Rust Skia matches exact weight/italic faces. */
  fontPaths: string[];
  text: string;
  fontSize: number;
  /** Minimum CSS height while the first frame loads. */
  minHeight?: number;
  /** Maximum CSS height for auto-sized wrapped paragraphs (prevents unbounded growth). */
  maxHeight?: number;
  /** Optional explicit hex color; defaults to the host element's live computed CSS color. */
  colorHex?: string;
  /** Pass preview/app theme key so changing Paper/Ink/Dark re-samples ink color immediately. */
  themeKey?: string;
  features?: Record<string, number>;
  variations?: Record<string, number>;
  weight?: number;
  italic?: boolean;
  align?: "left" | "center" | "right";
  lineHeight?: number;
  letterSpacing?: number;
  className?: string;
  /** Invoked when Rust detects a color/emoji table (COLR/CBDT/SVG/sbix) so UI can switch to native DOM color. */
  onColorFallback?: () => void;
}

/**
 * Live HTML5 `<canvas>` specimen renderer backed by the Rust Skia (FreeType + HarfBuzz) worker.
 *
 * Uses single-flight latest-wins scheduling: at most one IPC render is in flight per canvas,
 * and any slider/toggle changes during that frame overwrite the pending snapshot so the next
 * frame fires immediately with zero debounce lag and zero backlog.
 */
export const SkiaFontCanvas = memo(function SkiaFontCanvas({
  slot = "inspector-main",
  fontPaths,
  text,
  fontSize,
  minHeight = 44,
  maxHeight = 360,
  colorHex,
  themeKey,
  features,
  variations,
  weight,
  italic = false,
  align = "left",
  lineHeight = 1.3,
  letterSpacing = 0,
  className,
  onColorFallback,
}: SkiaFontCanvasProps) {
  const hostRef = useRef<HTMLDivElement | null>(null);
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const [width, setWidth] = useState(0);
  const [cssHeight, setCssHeight] = useState(minHeight);
  const [visible, setVisible] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [docThemeTick, setDocThemeTick] = useState(0);

  const pathsKey = useMemo(() => fontPaths.join("\0"), [fontPaths]);
  const featuresKey = useMemo(() => JSON.stringify(features ?? {}), [features]);
  const variationsKey = useMemo(() => JSON.stringify(variations ?? {}), [variations]);

  const inFlightRef = useRef(false);
  const pendingRef = useRef<RenderFontSpecimenOptions | null>(null);
  const mountedRef = useRef(true);
  const renderEpochRef = useRef(0);
  const colorFallbackRef = useRef(onColorFallback);
  colorFallbackRef.current = onColorFallback;

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      pendingRef.current = null;
    };
  }, []);

  // Re-sample computed ink color when document light/dark theme class changes.
  useEffect(() => {
    if (colorHex || typeof MutationObserver === "undefined") return;
    const root = document.documentElement;
    const mo = new MutationObserver(() => setDocThemeTick((t) => t + 1));
    mo.observe(root, { attributes: true, attributeFilter: ["class", "data-theme"] });
    return () => mo.disconnect();
  }, [colorHex]);

  useEffect(() => {
    const el = hostRef.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    const update = () => {
      const next = Math.max(80, Math.floor(el.clientWidth));
      setWidth((prev) => (Math.abs(prev - next) >= 2 ? next : prev));
    };
    update();
    const ro = new ResizeObserver(update);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  useEffect(() => {
    const el = hostRef.current;
    if (!el || typeof IntersectionObserver === "undefined") return;
    const io = new IntersectionObserver(
      (entries) => {
        if (entries[0]) setVisible(entries[0].isIntersecting);
      },
      { rootMargin: "240px 0px" },
    );
    io.observe(el);
    return () => io.disconnect();
  }, []);

  useEffect(() => {
    if (!visible || width <= 0 || !pathsKey) return;
    const paths = pathsKey.split("\0").filter(Boolean);
    if (!paths.length) return;

    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    const pixelW = Math.max(1, Math.round(width * dpr));
    const pixelMaxH = Math.max(1, Math.round(maxHeight * dpr));
    const host = hostRef.current;
    const resolvedHex =
      colorHex || (host ? cssColorToHex(getComputedStyle(host).color, "#111111") : "#111111");

    const parsedFeatures = JSON.parse(featuresKey) as Record<string, number>;
    const parsedVariations = JSON.parse(variationsKey) as Record<string, number>;

    const req: RenderFontSpecimenOptions = {
      slot,
      fontPaths: paths,
      text: text || "The quick brown fox jumps over the lazy dog 0123456789",
      fontSize: fontSize * dpr,
      width: pixelW,
      maxHeight: pixelMaxH,
      colorHex: resolvedHex,
      features: parsedFeatures,
      variations: parsedVariations,
      weight,
      italic,
      align,
      lineHeight,
      letterSpacing,
    };

    const epoch = ++renderEpochRef.current;
    pendingRef.current = req;

    const drain = () => {
      if (inFlightRef.current || !mountedRef.current) return;
      const next = pendingRef.current;
      if (!next) return;
      pendingRef.current = null;
      inFlightRef.current = true;
      const reqDpr = Math.min(window.devicePixelRatio || 1, 2);

      void renderFontSpecimen(next)
        .then((frame) => {
          if (!mountedRef.current) return;
          // Drop stale frame if a newer request is already queued.
          if (pendingRef.current) return;
          const canvas = canvasRef.current;
          if (!canvas) return;
          const buffer = frame.pixels;
          canvas.width = frame.width;
          canvas.height = frame.height;
          const ctx = canvas.getContext("2d");
          if (!ctx) return;
          const imageData = new ImageData(new Uint8ClampedArray(buffer), frame.width, frame.height);
          ctx.putImageData(imageData, 0, 0);
          const nextCssH = Math.max(minHeight, Math.ceil(frame.height / reqDpr));
          setCssHeight(nextCssH);
          setError(null);
        })
        .catch((err: unknown) => {
          if (!mountedRef.current || epoch !== renderEpochRef.current) return;
          const msg = err instanceof Error ? err.message : String(err);
          if (msg.includes(SKIA_SUPERSEDED)) return;
          if (msg.includes("color-font-dom-fallback")) {
            setError(null);
            colorFallbackRef.current?.();
            return;
          }
          setError(msg);
        })
        .finally(() => {
          inFlightRef.current = false;
          if (mountedRef.current && pendingRef.current) {
            drain();
          }
        });
    };

    drain();
  }, [
    visible,
    slot,
    pathsKey,
    text,
    fontSize,
    width,
    minHeight,
    maxHeight,
    colorHex,
    themeKey,
    docThemeTick,
    featuresKey,
    variationsKey,
    weight,
    italic,
    align,
    lineHeight,
    letterSpacing,
  ]);

  return (
    <div
      ref={hostRef}
      className={cn("relative w-full min-w-0 max-w-full overflow-hidden", className)}
      style={{ minHeight: `${minHeight}px`, height: `${cssHeight}px` }}
    >
      <canvas
        ref={canvasRef}
        data-testid="skia-font-canvas"
        aria-label={`Fontations preview of ${fontPaths[0] ?? "font"}`}
        className="block h-full w-full"
        style={{ width: "100%", height: `${cssHeight}px` }}
      />
      {error ? (
        <span className="pointer-events-none absolute inset-0 flex items-center text-xs text-destructive">
          {error}
        </span>
      ) : null}
    </div>
  );
});
