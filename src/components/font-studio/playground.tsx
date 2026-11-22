import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { ArrowLeftRight, ChevronDown, ChevronUp, Columns2, Copy, RotateCcw, Sparkles, Square, ZoomIn } from "lucide-react";
import { toast } from "sonner";
import { copyText } from "@/lib/copy-text";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import { Slider } from "@/components/ui/slider";
import { FontPicker } from "./font-picker";
import { googleFontId } from "@/lib/fonts/catalog";
import { cssFamilyStack, loadFont, loadFontWeight } from "@/lib/fonts/loader";
import { loadSystemFonts } from "@/lib/fonts/system-fonts";
import { synthesisForFont } from "@/lib/fonts/synthesis";
import { allFonts, findFont, useFontStore } from "@/lib/fonts/store";
import {
  axesForFont,
  defaultAxisValues,
  defaultWeightForFont,
  instancesForFont,
  resolvedAxisValues,
  variationStyle,
} from "@/lib/fonts/axes";
import {
  computePairingMatrix,
  PAIRING_PRESETS,
  pickNextCompanion,
  suggestCompanionCandidates,
  type PairingPreset,
} from "@/lib/fonts/pairing";
import { AxisSliders } from "./axis-sliders";
import { cn } from "@/lib/utils";

const HEADING_SAMPLE = "Display the story, then set the body.";
const BODY_SAMPLE =
  "A pairing works when contrast is clear and rhythm agrees. Try a display serif against a quiet sans, or a geometric headline with a humanist paragraph. Adjust size, line height, and tracking until the two speak in one voice.";

const PLAYGROUND_STORAGE_KEY = "font-manager:playground-v1";

const OT_TOGGLES: { tag: string; label: string; hint: string }[] = [
  { tag: "liga", label: "liga", hint: "Standard ligatures (fi, fl)" },
  { tag: "dlig", label: "dlig", hint: "Discretionary ligatures" },
  { tag: "smcp", label: "smcp", hint: "Small capitals" },
  { tag: "tnum", label: "tnum", hint: "Tabular figures" },
  { tag: "onum", label: "onum", hint: "Oldstyle figures" },
  { tag: "zero", label: "zero", hint: "Slashed zero" },
  { tag: "frac", label: "frac", hint: "Diagonal fractions" },
  { tag: "ss01", label: "ss01", hint: "Stylistic set 01" },
];

type PlaygroundSnapshot = {
  leftId?: string;
  rightId?: string;
  leftSize?: number;
  rightSize?: number;
  leftWeight?: number;
  rightWeight?: number;
  leftLineHeight?: number;
  rightLineHeight?: number;
  leftTracking?: number;
  rightTracking?: number;
  leftAxes?: Record<string, number>;
  rightAxes?: Record<string, number>;
  heading?: string;
  body?: string;
  invert?: boolean;
  fgColor?: string;
  bgColor?: string;
  zoom?: number;
  otFeatures?: Record<string, boolean>;
};

function loadPlaygroundSnapshot(): PlaygroundSnapshot {
  try {
    if (typeof window === "undefined") return {};
    const raw = window.localStorage.getItem(PLAYGROUND_STORAGE_KEY);
    if (!raw) return {};
    const parsed = JSON.parse(raw) as unknown;
    return parsed && typeof parsed === "object" ? (parsed as PlaygroundSnapshot) : {};
  } catch {
    return {};
  }
}

function buildFeatureSettings(map: Record<string, boolean>): string | undefined {
  const parts: string[] = [];
  for (const item of OT_TOGGLES) {
    if (map[item.tag] === true) parts.push(`"${item.tag}" 1`);
    else if (map[item.tag] === false && item.tag === "liga") parts.push(`"liga" 0`);
  }
  return parts.length ? parts.join(", ") : undefined;
}

/** Auto-growing textarea that never clips or spawns nested scrollbars at large display sizes. */
function AutoTextarea({
  value,
  onChange,
  "aria-label": ariaLabel,
  className,
  style,
}: {
  value: string;
  onChange: (next: string) => void;
  "aria-label": string;
  className?: string;
  style?: React.CSSProperties;
}) {
  const ref = useRef<HTMLTextAreaElement>(null);

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.max(el.scrollHeight, 36)}px`;
  }, [
    value,
    style?.fontFamily,
    style?.fontSize,
    style?.lineHeight,
    style?.letterSpacing,
    style?.fontWeight,
    style?.fontVariationSettings,
  ]);

  return (
    <textarea
      ref={ref}
      value={value}
      aria-label={ariaLabel}
      rows={1}
      onChange={(e) => onChange(e.target.value)}
      className={cn(
        "w-full resize-none overflow-hidden break-words bg-transparent [overflow-wrap:anywhere]",
        className,
      )}
      style={style}
    />
  );
}

export function Playground() {
  const hydrated = useFontStore((s) => s.hydrated);
  const localFonts = useFontStore((s) => s.localFonts);
  const googleFonts = useFontStore((s) => s.googleFonts);
  const systemFonts = useFontStore((s) => s.systemFonts);
  const activatedSet = useFontStore((s) => s.activatedSet);
  const activatedFonts = useMemo(
    () =>
      allFonts(localFonts, googleFonts)
        .filter((font) => activatedSet.has(font.id))
        .sort((a, b) => a.family.localeCompare(b.family)),
    [localFonts, googleFonts, activatedSet],
  );
  const [snap] = useState(loadPlaygroundSnapshot);
  const [leftId, setLeftId] = useState(
    () => (typeof snap.leftId === "string" && snap.leftId) || googleFontId("Playfair Display"),
  );
  const [rightId, setRightId] = useState(
    () => (typeof snap.rightId === "string" && snap.rightId) || googleFontId("Source Sans 3"),
  );
  const [leftSize, setLeftSize] = useState(
    () => (typeof snap.leftSize === "number" && snap.leftSize >= 12 ? snap.leftSize : 48),
  );
  const [rightSize, setRightSize] = useState(
    () => (typeof snap.rightSize === "number" && snap.rightSize >= 12 ? snap.rightSize : 18),
  );
  const [leftWeight, setLeftWeight] = useState(
    () => (typeof snap.leftWeight === "number" && snap.leftWeight >= 100 ? snap.leftWeight : 700),
  );
  const [rightWeight, setRightWeight] = useState(
    () => (typeof snap.rightWeight === "number" && snap.rightWeight >= 100 ? snap.rightWeight : 400),
  );
  const [leftLineHeight, setLeftLineHeight] = useState(
    () => (typeof snap.leftLineHeight === "number" && snap.leftLineHeight >= 0.8 ? snap.leftLineHeight : 1.15),
  );
  const [rightLineHeight, setRightLineHeight] = useState(
    () => (typeof snap.rightLineHeight === "number" && snap.rightLineHeight >= 0.8 ? snap.rightLineHeight : 1.55),
  );
  const [leftTracking, setLeftTracking] = useState(
    () => (typeof snap.leftTracking === "number" ? snap.leftTracking : 0),
  );
  const [rightTracking, setRightTracking] = useState(
    () => (typeof snap.rightTracking === "number" ? snap.rightTracking : 0),
  );
  const [leftAxes, setLeftAxes] = useState<Record<string, number>>(
    () => (snap.leftAxes && typeof snap.leftAxes === "object" ? snap.leftAxes : {}),
  );
  const [rightAxes, setRightAxes] = useState<Record<string, number>>(
    () => (snap.rightAxes && typeof snap.rightAxes === "object" ? snap.rightAxes : {}),
  );
  const [heading, setHeading] = useState(
    () => (typeof snap.heading === "string" ? snap.heading : HEADING_SAMPLE),
  );
  const [body, setBody] = useState(
    () => (typeof snap.body === "string" ? snap.body : BODY_SAMPLE),
  );
  const [invert, setInvert] = useState(() => Boolean(snap.invert));
  const [fgColor, setFgColor] = useState(() => (typeof snap.fgColor === "string" ? snap.fgColor : ""));
  const [bgColor, setBgColor] = useState(() => (typeof snap.bgColor === "string" ? snap.bgColor : ""));
  const [zoom, setZoom] = useState(
    () => (typeof snap.zoom === "number" && snap.zoom >= 0.5 && snap.zoom <= 2 ? snap.zoom : 1),
  );
  const [controlsCollapsed, setControlsCollapsed] = useState(false);
  const [singlePane, setSinglePane] = useState(false);
  const [otFeatures, setOtFeatures] = useState<Record<string, boolean>>(
    () => (snap.otFeatures && typeof snap.otFeatures === "object" ? snap.otFeatures : { liga: true }),
  );

  // Ensure System Fonts load reactively if a saved or chosen pair uses an OS font (`s:*`).
  useEffect(() => {
    if ((leftId.startsWith("s:") || rightId.startsWith("s:")) && systemFonts.length === 0) {
      void loadSystemFonts();
    }
  }, [leftId, rightId, systemFonts.length]);

  const left = useMemo(
    () =>
      leftId.startsWith("s:")
        ? systemFonts.find((f) => f.id === leftId)
        : findFont(leftId, localFonts, googleFonts),
    [leftId, localFonts, googleFonts, systemFonts],
  );
  const right = useMemo(
    () =>
      rightId.startsWith("s:")
        ? systemFonts.find((f) => f.id === rightId)
        : findFont(rightId, localFonts, googleFonts),
    [rightId, localFonts, googleFonts, systemFonts],
  );

  const leftRef = useRef(left);
  leftRef.current = left;
  const rightRef = useRef(right);
  rightRef.current = right;
  const keepLeftPresetRef = useRef(
    Boolean(snap.leftId && (snap.leftWeight !== undefined || (snap.leftAxes && Object.keys(snap.leftAxes).length))),
  );
  const keepRightPresetRef = useRef(
    Boolean(snap.rightId && (snap.rightWeight !== undefined || (snap.rightAxes && Object.keys(snap.rightAxes).length))),
  );

  // Debounced persistence (250ms) so dragging sliders at 60fps never blocks on synchronous JSON I/O.
  // Font ID changes flush immediately so closing right after picking a typeface always persists.
  const persistPayload = useMemo(
    () => ({
      leftId,
      rightId,
      leftSize,
      rightSize,
      leftWeight,
      rightWeight,
      leftLineHeight,
      rightLineHeight,
      leftTracking,
      rightTracking,
      leftAxes,
      rightAxes,
      heading,
      body,
      invert,
      fgColor,
      bgColor,
      zoom,
      otFeatures,
    }),
    [
      leftId,
      rightId,
      leftSize,
      rightSize,
      leftWeight,
      rightWeight,
      leftLineHeight,
      rightLineHeight,
      leftTracking,
      rightTracking,
      leftAxes,
      rightAxes,
      heading,
      body,
      invert,
      fgColor,
      bgColor,
      zoom,
      otFeatures,
    ],
  );

  const lastSavedIdsRef = useRef(`${leftId}|${rightId}`);
  useEffect(() => {
    const write = () => {
      try {
        window.localStorage.setItem(PLAYGROUND_STORAGE_KEY, JSON.stringify(persistPayload));
      } catch {
        /* ignore quota/private mode */
      }
    };
    const idKey = `${leftId}|${rightId}`;
    if (idKey !== lastSavedIdsRef.current) {
      lastSavedIdsRef.current = idKey;
      write();
      return;
    }
    const timer = window.setTimeout(write, 250);
    return () => window.clearTimeout(timer);
  }, [persistPayload, leftId, rightId]);

  useEffect(() => {
    const leftOk =
      (!hydrated && leftId.startsWith("l-")) ||
      leftId.startsWith("s:") ||
      Boolean(findFont(leftId, localFonts, googleFonts));
    const rightOk =
      (!hydrated && rightId.startsWith("l-")) ||
      rightId.startsWith("s:") ||
      Boolean(findFont(rightId, localFonts, googleFonts));
    if (leftOk && rightOk) return;
    if (!activatedFonts.length) return;
    if (!leftOk) setLeftId(activatedFonts[0]!.id);
    if (!rightOk) setRightId((activatedFonts[1] ?? activatedFonts[0])!.id);
  }, [hydrated, activatedFonts, localFonts, googleFonts, leftId, rightId]);

  useEffect(() => {
    if (left) void loadFont(left, "full");
  }, [left]);
  useEffect(() => {
    if (right) void loadFont(right, "full");
  }, [right]);

  useEffect(() => {
    const cur = leftRef.current;
    setSuggestIdx(0);
    if (!cur) return;
    void loadFont(cur, "full");
    if (keepLeftPresetRef.current) {
      keepLeftPresetRef.current = false;
      return;
    }
    setLeftAxes(defaultAxisValues(axesForFont(cur), instancesForFont(cur)));
    setLeftWeight(defaultWeightForFont(cur));
  }, [leftId]);

  useEffect(() => {
    const cur = rightRef.current;
    if (!cur) return;
    void loadFont(cur, "full");
    if (keepRightPresetRef.current) {
      keepRightPresetRef.current = false;
      return;
    }
    setRightAxes(defaultAxisValues(axesForFont(cur), instancesForFont(cur)));
    setRightWeight(defaultWeightForFont(cur));
  }, [rightId]);

  const featureSettings = useMemo(() => buildFeatureSettings(otFeatures), [otFeatures]);

  const css = useMemo(() => {
    if (!left || !right) return "";
    const leftAxesResolved = resolvedAxisValues(axesForFont(left), { ...leftAxes, wght: leftAxes.wght ?? leftWeight });
    const rightAxesResolved = resolvedAxisValues(axesForFont(right), { ...rightAxes, wght: rightAxes.wght ?? rightWeight });
    const leftStyle = variationStyle(leftAxesResolved, axesForFont(left));
    const rightStyle = variationStyle(rightAxesResolved, axesForFont(right));
    const fmtExtra = (st: ReturnType<typeof variationStyle>, lh: number, trk: number) => {
      const lines: string[] = [
        `  line-height: ${lh.toFixed(2)};`,
        ...(trk !== 0 ? [`  letter-spacing: ${trk.toFixed(3)}em;`] : []),
        ...(featureSettings ? [`  font-feature-settings: ${featureSettings};`] : []),
      ];
      if (st.fontStyle && st.fontStyle !== "normal") lines.push(`  font-style: ${st.fontStyle};`);
      if (st.fontStretch) lines.push(`  font-stretch: ${st.fontStretch};`);
      if (st.fontOpticalSizing) lines.push(`  font-optical-sizing: ${st.fontOpticalSizing};`);
      return lines.length ? `\n${lines.join("\n")}` : "";
    };
    return `h1 {\n  font-family: ${cssFamilyStack(left)};\n  font-weight: ${Math.round(leftAxesResolved.wght ?? leftWeight)};\n  font-size: ${leftSize}px;\n  font-variation-settings: ${leftStyle.fontVariationSettings};${fmtExtra(leftStyle, leftLineHeight, leftTracking)}\n}\n\np {\n  font-family: ${cssFamilyStack(right)};\n  font-weight: ${Math.round(rightAxesResolved.wght ?? rightWeight)};\n  font-size: ${rightSize}px;\n  font-variation-settings: ${rightStyle.fontVariationSettings};${fmtExtra(rightStyle, rightLineHeight, rightTracking)}\n}`;
  }, [
    left,
    right,
    leftSize,
    rightSize,
    leftWeight,
    rightWeight,
    leftLineHeight,
    rightLineHeight,
    leftTracking,
    rightTracking,
    leftAxes,
    rightAxes,
    featureSettings,
  ]);

  const [suggestIdx, setSuggestIdx] = useState(0);

  function swap() {
    keepLeftPresetRef.current = true;
    keepRightPresetRef.current = true;
    setLeftId(rightId);
    setRightId(leftId);
    setLeftSize(rightSize);
    setRightSize(leftSize);
    setLeftWeight(rightWeight);
    setRightWeight(leftWeight);
    setLeftLineHeight(rightLineHeight);
    setRightLineHeight(leftLineHeight);
    setLeftTracking(rightTracking);
    setRightTracking(leftTracking);
    setLeftAxes(rightAxes);
    setRightAxes(leftAxes);
  }

  function applyPreset(preset: PairingPreset) {
    const pool = allFonts(localFonts, googleFonts);
    const hFont =
      pool.find((f) => f.family.toLowerCase() === preset.headingFamily.toLowerCase()) ??
      findFont(googleFontId(preset.headingFamily), localFonts, googleFonts);
    const bFont =
      pool.find((f) => f.family.toLowerCase() === preset.bodyFamily.toLowerCase()) ??
      findFont(googleFontId(preset.bodyFamily), localFonts, googleFonts);
    if (hFont) {
      keepLeftPresetRef.current = hFont.id !== leftId;
      setLeftId(hFont.id);
      setLeftAxes({ ...defaultAxisValues(axesForFont(hFont), instancesForFont(hFont)), wght: preset.headingWeight });
    }
    if (bFont) {
      keepRightPresetRef.current = bFont.id !== rightId;
      setRightId(bFont.id);
      setRightAxes({ ...defaultAxisValues(axesForFont(bFont), instancesForFont(bFont)), wght: preset.bodyWeight });
    }
    setLeftSize(preset.headingSize);
    setRightSize(preset.bodySize);
    setLeftWeight(preset.headingWeight);
    setRightWeight(preset.bodyWeight);
  }

  function suggestCompanion() {
    if (!left) return;
    const allPool = allFonts(localFonts, googleFonts);
    const pool = allPool.length >= 2 ? allPool : activatedFonts;
    const candidates = suggestCompanionCandidates({
      left,
      pool,
      activatedIds: activatedSet,
      leftSize,
      rightSize,
      leftWeight: leftAxes.wght ?? leftWeight,
    });
    const pick = pickNextCompanion(candidates, suggestIdx, rightId);
    if (!pick) return;
    setSuggestIdx((n) => n + 1);
    keepRightPresetRef.current = pick.font.id !== rightId;
    setRightId(pick.font.id);
    const nextWeight = pick.font.weights.includes(400) ? 400 : defaultWeightForFont(pick.font);
    setRightWeight(nextWeight);
    setRightAxes({
      ...defaultAxisValues(axesForFont(pick.font), instancesForFont(pick.font)),
      wght: nextWeight,
    });
    toast.message(`Paired ${left.family} with ${pick.font.family}`, {
      id: "fm-pair-suggest",
      description: `Score ${pick.score}/100 · ${pick.reason}`,
    });
  }

  const matrix = useMemo(
    () =>
      computePairingMatrix({
        left,
        right,
        leftSize,
        rightSize,
        leftWeight: leftAxes.wght ?? leftWeight,
        rightWeight: rightAxes.wght ?? rightWeight,
      }),
    [left, right, leftSize, rightSize, leftWeight, rightWeight, leftAxes.wght, rightAxes.wght],
  );

  // Full variationStyle (incl. fontOpticalSizing when opsz is in FVS) for both panes × edit/preview.
  // Check catalogVariable & axesForFont too so variable Google Fonts animate immediately even before
  // on-disk fvar patching flips `font.variable`.
  const leftAxesList = useMemo(() => (left ? axesForFont(left) : []), [left]);
  const rightAxesList = useMemo(() => (right ? axesForFont(right) : []), [right]);
  const leftVarStyle = left?.variable || left?.catalogVariable || leftAxesList.length > 0
    ? variationStyle({ ...leftAxes, wght: leftAxes.wght ?? leftWeight }, leftAxesList)
    : null;
  const rightVarStyle = right?.variable || right?.catalogVariable || rightAxesList.length > 0
    ? variationStyle({ ...rightAxes, wght: rightAxes.wght ?? rightWeight }, rightAxesList)
    : null;

  const hasCustomColors = Boolean(fgColor || bgColor);
  const primarySurfaceStyle: React.CSSProperties = hasCustomColors
    ? {
        color: invert ? bgColor || "#ffffff" : fgColor || "#111111",
        backgroundColor: invert ? fgColor || "#111111" : bgColor || "#ffffff",
      }
    : {};
  const invertedSurfaceStyle: React.CSSProperties = hasCustomColors
    ? {
        color: invert ? fgColor || "#111111" : bgColor || "#ffffff",
        backgroundColor: invert ? bgColor || "#ffffff" : fgColor || "#111111",
      }
    : {};

  const surface = hasCustomColors ? "" : invert ? "bg-ink text-paper" : "bg-paper text-ink";

  const headingBaseStyle: React.CSSProperties = {
    fontFamily: left ? cssFamilyStack(left) : undefined,
    fontSize: leftSize,
    lineHeight: leftLineHeight,
    letterSpacing: `${leftTracking}em`,
    fontWeight: leftAxes.wght ?? leftWeight,
    fontSynthesis: left ? synthesisForFont(left, { weight: leftAxes.wght ?? leftWeight }) : "weight",
    fontFeatureSettings: featureSettings,
  };

  const bodyBaseStyle: React.CSSProperties = {
    fontFamily: right ? cssFamilyStack(right) : undefined,
    fontSize: rightSize,
    lineHeight: rightLineHeight,
    letterSpacing: `${rightTracking}em`,
    fontWeight: rightAxes.wght ?? rightWeight,
    fontSynthesis: right ? synthesisForFont(right, { weight: rightAxes.wght ?? rightWeight }) : "weight",
    fontFeatureSettings: featureSettings,
  };

  const zoomWrapperStyle: React.CSSProperties =
    zoom === 1
      ? {}
      : {
          transform: `scale(${zoom})`,
          transformOrigin: "top left",
          width: `${100 / zoom}%`,
        };

  return (
    <div className="flex min-h-0 flex-1 flex-col overflow-hidden">
      {!controlsCollapsed ? (
        <div
          data-testid="playground-top-controls"
          className="fm-scroll grid max-h-[42vh] shrink-0 gap-4 overflow-y-auto overscroll-contain border-b border-border p-4 md:grid-cols-[1fr_auto_1fr] md:p-5"
        >
        <PaneControls
          label="Heading"
          fontId={leftId}
          onFont={setLeftId}
          size={leftSize}
          onSize={setLeftSize}
          lineHeight={leftLineHeight}
          onLineHeight={setLeftLineHeight}
          tracking={leftTracking}
          onTracking={setLeftTracking}
          weight={leftWeight}
          onWeight={(w) => {
            setLeftWeight(w);
            setLeftAxes((prev) => ("wght" in prev ? { ...prev, wght: w } : prev));
          }}
          weights={left?.weights ?? [400, 700]}
          font={left}
          axes={left ? axesForFont(left) : []}
          instances={left ? instancesForFont(left) : []}
          axisValues={leftAxes}
          onAxis={(tag, value) => {
            setLeftAxes((prev) => ({ ...prev, [tag]: value }));
            if (tag === "wght") {
              setLeftWeight(Math.round(value));
              if (left) void loadFontWeight(left, Math.round(value));
            }
          }}
        />
        <div className="flex items-center justify-center">
          <Button variant="ghost" size="icon" aria-label="Swap fonts" onClick={swap}>
            <ArrowLeftRight />
          </Button>
        </div>
        <PaneControls
          label="Body"
          fontId={rightId}
          onFont={setRightId}
          size={rightSize}
          onSize={setRightSize}
          lineHeight={rightLineHeight}
          onLineHeight={setRightLineHeight}
          tracking={rightTracking}
          onTracking={setRightTracking}
          weight={rightWeight}
          onWeight={(w) => {
            setRightWeight(w);
            setRightAxes((prev) => ("wght" in prev ? { ...prev, wght: w } : prev));
          }}
          weights={right?.weights ?? [400, 700]}
          font={right}
          axes={right ? axesForFont(right) : []}
          instances={right ? instancesForFont(right) : []}
          axisValues={rightAxes}
          onAxis={(tag, value) => {
            setRightAxes((prev) => ({ ...prev, [tag]: value }));
            if (tag === "wght") {
              setRightWeight(Math.round(value));
              if (right) void loadFontWeight(right, Math.round(value));
            }
          }}
        />
        </div>
      ) : null}

      <div
        className="flex shrink-0 flex-wrap items-center justify-between gap-2 border-b border-border bg-secondary/40 px-4 py-2 text-xs md:px-6"
        aria-label="Font pairing and contrast matrix"
      >
        <div className="flex flex-wrap items-center gap-2">
          <Badge variant={matrix.score >= 80 ? "default" : "outline"} className="tabular-nums">
            {matrix.verdict} · {matrix.score}/100
          </Badge>
          <span className="text-muted-foreground" title="Classification and structural contrast">
            {matrix.catRel.label}
          </span>
          <span className="text-muted-foreground">·</span>
          <span className="tabular-nums text-muted-foreground" title="Heading to body size ratio">
            Scale {matrix.ratio.toFixed(2)}× ({matrix.ratioTier})
          </span>
          <span className="text-muted-foreground">·</span>
          <span className="tabular-nums text-muted-foreground" title="Weight contrast between heading and body">
            Weight Δ{matrix.weightDelta} ({matrix.weightTier})
          </span>
        </div>
        <div className="flex flex-wrap items-center gap-1.5">
          {PAIRING_PRESETS.map((preset) => (
            <Button
              key={preset.id}
              type="button"
              size="sm"
              variant="ghost"
              className="h-6 px-2 text-[11px]"
              title={`${preset.headingFamily} + ${preset.bodyFamily}`}
              onClick={() => applyPreset(preset)}
            >
              {preset.label}
            </Button>
          ))}
          <Button
            type="button"
            size="sm"
            variant="secondary"
            className="h-6 px-2 text-[11px]"
            onClick={suggestCompanion}
          >
            <Sparkles className="size-3" />
            Suggest pair
          </Button>
          <Button
            type="button"
            size="sm"
            variant="outline"
            className="h-6 px-2 text-[11px]"
            data-testid="playground-layout-toggle"
            title={singlePane ? "Show split light/dark comparison" : "Expand to single full-width viewport"}
            onClick={() => setSinglePane((v) => !v)}
          >
            {singlePane ? <Columns2 className="size-3" /> : <Square className="size-3" />}
            {singlePane ? "Split" : "Single"}
          </Button>
          <Button
            type="button"
            size="sm"
            variant="outline"
            className="h-6 px-2 text-[11px]"
            data-testid="playground-toggle-controls"
            title={controlsCollapsed ? "Show font controls" : "Collapse controls to maximize viewport"}
            onClick={() => setControlsCollapsed((v) => !v)}
          >
            {controlsCollapsed ? <ChevronDown className="size-3" /> : <ChevronUp className="size-3" />}
            {controlsCollapsed ? "Show controls" : "Focus viewport"}
          </Button>
        </div>
      </div>

      <div
        className={cn(
          "fm-scroll grid min-h-[260px] flex-1 gap-px overflow-auto bg-border",
          !singlePane && "lg:grid-cols-2",
        )}
        tabIndex={0} role="group" aria-label="Pairing preview">
        <article
          className={cn("fm-scroll min-h-0 overflow-y-auto overscroll-contain p-6 md:p-10", surface)}
          style={primarySurfaceStyle}
          data-testid="playground-viewport-primary"
        >
          <div style={zoomWrapperStyle}>
            <AutoTextarea
              value={heading}
              onChange={setHeading}
              aria-label="Heading sample"
              className="outline-none focus-visible:ring-2 focus-visible:ring-ring"
              style={{
                ...headingBaseStyle,
                ...(leftVarStyle ?? {}),
              }}
            />
            <AutoTextarea
              value={body}
              onChange={setBody}
              aria-label="Body sample"
              className="mt-4 outline-none focus-visible:ring-2 focus-visible:ring-ring"
              style={{
                ...bodyBaseStyle,
                ...(rightVarStyle ?? {}),
              }}
            />
          </div>
        </article>
        {!singlePane ? (
        <article
          tabIndex={0}
          role="region"
          aria-label="Pairing preview (inverted)"
          className={cn(
            "fm-scroll min-h-0 overflow-y-auto overscroll-contain p-6 outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-offset-1 md:p-10",
            !hasCustomColors &&
              (invert
                ? "bg-paper text-ink focus-visible:ring-ink focus-visible:ring-offset-paper"
                : "bg-ink text-paper focus-visible:ring-paper focus-visible:ring-offset-ink"),
          )}
          style={invertedSurfaceStyle}
          data-testid="playground-viewport-inverted"
        >
          <div style={zoomWrapperStyle}>
            <p
              className="break-words [overflow-wrap:anywhere]"
              style={{
                ...headingBaseStyle,
                ...(leftVarStyle ?? {}),
              }}
            >
              {heading}
            </p>
            <p
              className="mt-4 max-w-prose break-words [overflow-wrap:anywhere]"
              style={{
                ...bodyBaseStyle,
                ...(rightVarStyle ?? {}),
              }}
            >
              {body}
            </p>
          </div>
        </article>
        ) : null}
      </div>

      <div className="flex shrink-0 flex-wrap items-center gap-x-3 gap-y-2 border-t border-border px-4 py-2.5 md:px-6">
        <div className="flex items-center gap-1.5" role="group" aria-label="Surface colors">
          <Button size="sm" variant="secondary" className="h-7 px-2.5 text-xs" onClick={() => setInvert((v) => !v)}>
            Flip paper
          </Button>
          <label className="inline-flex items-center gap-1 text-[11px] text-muted-foreground" title="Text color">
            <span>Ink</span>
            <input
              type="color"
              aria-label="Text color"
              data-testid="playground-fg-color"
              value={fgColor || "#111111"}
              onChange={(e) => setFgColor(e.target.value)}
              className="h-6 w-6 cursor-pointer rounded border border-border bg-transparent p-0"
            />
          </label>
          <label className="inline-flex items-center gap-1 text-[11px] text-muted-foreground" title="Background color">
            <span>Paper</span>
            <input
              type="color"
              aria-label="Background color"
              data-testid="playground-bg-color"
              value={bgColor || "#ffffff"}
              onChange={(e) => setBgColor(e.target.value)}
              className="h-6 w-6 cursor-pointer rounded border border-border bg-transparent p-0"
            />
          </label>
          {hasCustomColors ? (
            <Button
              type="button"
              size="icon-sm"
              variant="ghost"
              className="size-6"
              aria-label="Reset colors"
              title="Reset surface colors"
              onClick={() => {
                setFgColor("");
                setBgColor("");
              }}
            >
              <RotateCcw className="size-3" />
            </Button>
          ) : null}
        </div>

        <div className="flex items-center gap-1.5" role="group" aria-label="Viewport zoom">
          <ZoomIn className="size-3.5 text-muted-foreground" />
          <div className="w-24">
            <Slider
              min={50}
              max={200}
              step={5}
              value={[Math.round(zoom * 100)]}
              aria-label="Viewport zoom"
              onValueChange={([v]) => {
                if (typeof v === "number") setZoom(v / 100);
              }}
            />
          </div>
          <button
            type="button"
            className="w-10 text-left font-mono text-[11px] tabular-nums text-muted-foreground hover:text-foreground"
            title="Click to reset zoom to 100%"
            onClick={() => setZoom(1)}
          >
            {Math.round(zoom * 100)}%
          </button>
        </div>

        <div className="flex flex-wrap items-center gap-1" role="group" aria-label="OpenType features">
          {OT_TOGGLES.map((feat) => {
            const on = Boolean(otFeatures[feat.tag]);
            return (
              <button
                key={feat.tag}
                type="button"
                aria-pressed={on}
                data-testid={`playground-ot-${feat.tag}`}
                title={feat.hint}
                onClick={() => setOtFeatures((prev) => ({ ...prev, [feat.tag]: !prev[feat.tag] }))}
                className={cn(
                  "h-6 rounded px-1.5 font-mono text-[11px] transition-colors",
                  on
                    ? "bg-primary text-primary-foreground"
                    : "bg-secondary text-muted-foreground hover:text-foreground",
                )}
              >
                {feat.label}
              </button>
            );
          })}
        </div>

        <div className="ml-auto flex items-center gap-2">
          <Button
            size="sm"
            variant="outline"
            className="h-7 px-2.5 text-xs"
            onClick={async () => {
              try {
                await copyText(css);
                toast.success("Pairing CSS copied", { id: "fm-pair-css" });
              } catch {
                toast.error("Clipboard blocked — select and press Ctrl+C");
              }
            }}
          >
            <Copy className="size-3" />
            Copy CSS
          </Button>
          <p className="hidden text-xs text-muted-foreground xl:block">
            {left?.family} + {right?.family}
          </p>
        </div>
      </div>
    </div>
  );
}

function PaneControls({
  label,
  fontId,
  onFont,
  size,
  onSize,
  lineHeight,
  onLineHeight,
  tracking,
  onTracking,
  weight,
  onWeight,
  weights,
  font,
  axes,
  instances = [],
  axisValues,
  onAxis,
}: {
  label: string;
  fontId: string;
  onFont: (id: string) => void;
  size: number;
  onSize: (n: number) => void;
  lineHeight: number;
  onLineHeight: (n: number) => void;
  tracking: number;
  onTracking: (n: number) => void;
  weight: number;
  onWeight: (n: number) => void;
  weights: number[];
  font?: ReturnType<typeof findFont>;
  axes: ReturnType<typeof axesForFont>;
  instances?: ReturnType<typeof instancesForFont>;
  axisValues: Record<string, number>;
  onAxis: (tag: string, value: number) => void;
}) {
  return (
    <div className="space-y-2.5">
      <div className="flex items-center justify-between gap-2">
        <Label>{label}</Label>
        <span className="font-mono text-[11px] tabular-nums text-muted-foreground">
          {size}px · LH {lineHeight.toFixed(2)} · {tracking >= 0 ? "+" : ""}
          {tracking.toFixed(2)}em
        </span>
      </div>
      <FontPicker value={fontId} onChange={onFont} label={label} />
      <div className="grid gap-2 sm:grid-cols-3">
        <div className="flex items-center gap-2">
          <span className="w-8 shrink-0 text-[11px] text-muted-foreground">Size</span>
          <Slider
            min={12}
            max={144}
            step={1}
            value={[size]}
            aria-label={`${label} size`}
            onValueChange={([v]) => {
              if (typeof v === "number") onSize(v);
            }}
          />
        </div>
        <div className="flex items-center gap-2">
          <span className="w-6 shrink-0 text-[11px] text-muted-foreground">LH</span>
          <Slider
            min={0.85}
            max={2.4}
            step={0.05}
            value={[lineHeight]}
            aria-label={`${label} line height`}
            onValueChange={([v]) => {
              if (typeof v === "number") onLineHeight(v);
            }}
          />
        </div>
        <div className="flex items-center gap-2">
          <span className="w-8 shrink-0 text-[11px] text-muted-foreground">Trk</span>
          <Slider
            min={-0.08}
            max={0.3}
            step={0.005}
            value={[tracking]}
            aria-label={`${label} tracking`}
            onValueChange={([v]) => {
              if (typeof v === "number") onTracking(v);
            }}
          />
        </div>
      </div>
      {axes.length ? (
        <AxisSliders axes={axes} values={axisValues} onChange={onAxis} instances={instances} />
      ) : (
        <div className="flex flex-wrap gap-1">
          {weights.map((w) => (
            <button
              key={w}
              type="button"
              onClick={() => {
                onWeight(w);
                if (font) void loadFontWeight(font, w);
              }}
              className={cn(
                "h-7 min-w-9 rounded-md px-2 font-mono text-xs tabular-nums",
                weight === w ? "bg-primary text-primary-foreground" : "bg-secondary text-muted-foreground",
              )}
            >
              {w}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
