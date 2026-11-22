/** c253 #1: our own marker writes (`.complete`, `.expected`, `.google-planned`,
 * `.fontsource-version`, …) inside the managed root must not re-kick a rescan —
 * that was the self-trigger loop. Any font file or folder path still syncs. */
export function isOwnMarkerOnlyEvent(ev: unknown): boolean {
  const paths = (ev as { paths?: unknown } | null | undefined)?.paths;
  if (!Array.isArray(paths) || paths.length === 0) return false;
  return paths.every((p) => {
    if (typeof p !== "string") return false;
    const base = p.replace(/[\\/]+$/, "").split(/[\\/]/).pop() ?? "";
    return base.startsWith(".") && base.length > 1;
  });
}
