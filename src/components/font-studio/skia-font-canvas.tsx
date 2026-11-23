import { memo, useEffect, useMemo, useRef, useState } from "react";
import { renderFontPreview } from "@/lib/fonts/skia-engine";
import { cn } from "@/lib/utils";

export interface SkiaFontCanvasProps {
  fontPath: string;
  text: string;
  fontSize: number;
  height?: number;
  colorHex?: string;
  features?: Record<string, number>;
  variations?: Record<string, number>;
  className?: string;
}

/**
 * HTML5 `<canvas>` specimen renderer backed by the Rust Skia (FreeType + HarfBuzz) worker thread.
 * Receives raw RGBA8888 pixel bytes over Tauri IPC and blits them directly via `ImageData` + `putImageData`.
 */
export const SkiaFontCanvas = memo(function SkiaFontCanvas({
  fontPath,
  text,
  fontSize,
  height = 72,
  colorHex = "#111111",
  features,
  variations,
  className,
}: SkiaFontCanvasProps) {
  const hostRef = useRef<HTMLDivElement | null>(null);
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const [width, setWidth] = useState(520);
  const [visible, setVisible] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const featuresKey = useMemo(() => JSON.stringify(features ?? {}), [features]);
  const variationsKey = useMemo(() => JSON.stringify(variations ?? {}), [variations]);

  useEffect(() => {
    const el = hostRef.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    const update = () => {
      const next = Math.max(120, Math.floor(el.clientWidth));
      setWidth((prev) => (Math.abs(prev - next) >= 4 ? next : prev));
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
    if (!visible || !fontPath) return;
    let cancelled = false;
    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    const pixelW = Math.max(1, Math.round(width * dpr));
    const pixelH = Math.max(1, Math.round(height * dpr));
    const parsedFeatures = JSON.parse(featuresKey) as Record<string, number>;
    const parsedVariations = JSON.parse(variationsKey) as Record<string, number>;

    void renderFontPreview({
      fontPath,
      text: text || "The quick brown fox jumps over the lazy dog 0123456789",
      fontSize: fontSize * dpr,
      width: pixelW,
      height: pixelH,
      colorHex,
      features: parsedFeatures,
      variations: parsedVariations,
    })
      .then((buffer) => {
        if (cancelled) return;
        const canvas = canvasRef.current;
        if (!canvas) return;
        const expectedBytes = pixelW * pixelH * 4;
        if (buffer.byteLength !== expectedBytes) {
          setError(`Unexpected RGBA buffer (${buffer.byteLength} != ${expectedBytes})`);
          return;
        }
        canvas.width = pixelW;
        canvas.height = pixelH;
        const ctx = canvas.getContext("2d");
        if (!ctx) return;
        const imageData = new ImageData(new Uint8ClampedArray(buffer), pixelW, pixelH);
        ctx.putImageData(imageData, 0, 0);
        setError(null);
      })
      .catch((err: unknown) => {
        if (cancelled) return;
        setError(err instanceof Error ? err.message : String(err));
      });

    return () => {
      cancelled = true;
    };
  }, [visible, fontPath, text, fontSize, width, height, colorHex, featuresKey, variationsKey]);

  return (
    <div
      ref={hostRef}
      className={cn("relative w-full overflow-hidden", className)}
      style={{ height }}
    >
      <canvas
        ref={canvasRef}
        data-testid="skia-font-canvas"
        aria-label={`Skia preview of ${fontPath}`}
        className="block h-full w-full"
        style={{ width: "100%", height: `${height}px` }}
      />
      {error ? (
        <span className="pointer-events-none absolute inset-0 flex items-center text-xs text-destructive">
          {error}
        </span>
      ) : null}
    </div>
  );
});
