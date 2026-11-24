import { useCallback, useEffect, useRef, useState, useSyncExternalStore, type ReactNode } from "react";
import { Link, useRouterState } from "@tanstack/react-router";
import { Code2, Copy, Grid3x3, HelpCircle, Library, Menu, Search, SplitSquareHorizontal, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Sheet, SheetContent } from "@/components/ui/sheet";
import { TooltipProvider } from "@/components/ui/tooltip";
import { toggleSearchToken } from "@/lib/fonts/metrics";
import { CollectionDialog } from "./collection-dialog";
import { CssExportDialog } from "./css-export-dialog";
import { DownloadBar } from "./download-bar";
import {
  getDownloadJob,
  cancelDocsVfSyncFromShortcut,
  isDocsVfSyncJob,
  subscribeDownloadJob,
} from "@/lib/fonts/os-activate";
import { shouldHideLibraryFromA11yDuringDocsJob } from "@/lib/fonts/docs-vf-sync-ownership.mjs";
import { FontInspector } from "./font-inspector";
import { Sidebar } from "./sidebar";
import { TabPanes } from "./tab-panes";
import { ThemeToggle } from "./theme-toggle";
import { NotificationCenter } from "./notification-center";
import { AppContextMenu } from "./context-menu";
import { shouldAllowNativeContextMenu } from "@/lib/context-menu-bus";
import { DesktopSettings } from "./desktop-settings";
import { HelpTip } from "./help-tip";
import { ActivateConfirmDialog, DeactivateConfirmDialog } from "./activate-confirm-dialog";
import { ImportCollisionDialog } from "./import-collision-dialog";
import { ImportOptionsDialog } from "./import-options-dialog";
import { runFontImport } from "./import-fonts";
import { useHydrateFonts } from "@/lib/fonts/hydrate";
import { useFontStore } from "@/lib/fonts/store";
import { cn } from "@/lib/utils";
import { APP_BUILD_SHA, APP_NAME, APP_VERSION } from "@/version";

const NAV = [
  { to: "/", label: "Library", icon: Library, match: (p: string) => p === "/", beta: false },
  {
    to: "/playground",
    label: "Playground",
    icon: SplitSquareHorizontal,
    match: (p: string) => p.startsWith("/playground"),
    beta: false,
  },
  {
    to: "/duplicates",
    label: "Duplicates",
    icon: Copy,
    match: (p: string) => p.startsWith("/duplicates"),
    beta: false,
  },
  {
    to: "/glyphs",
    label: "Glyphs",
    icon: Grid3x3,
    match: (p: string) => p.startsWith("/glyphs"),
    beta: false,
  },
] as const;

export function AppShell({ children: _children }: { children: ReactNode }) {
  useHydrateFonts();
  const pathname = useRouterState({ select: (s) => s.location.pathname });
  // 1.0.206y: shrink a11y tree during docs sync (library cards stay out of UIA Descendants).
  const job = useSyncExternalStore(subscribeDownloadJob, getDownloadJob, getDownloadJob);
  const hideLibraryA11y = shouldHideLibraryFromA11yDuringDocsJob({
    docsOwns: isDocsVfSyncJob(),
    cancelChromeEligible: job.running || job.paused,
  });
  const query = useFontStore((s) => s.query);
  const setQuery = useFontStore((s) => s.setQuery);
  const searchRef = useRef<HTMLInputElement>(null);
  const fileRef = useRef<HTMLInputElement>(null);
  const folderRef = useRef<HTMLInputElement>(null);
  const [mobileNav, setMobileNav] = useState(false);
  const [collectionOpen, setCollectionOpen] = useState(false);
  const [cssOpen, setCssOpen] = useState(false);
  const [inputsReady, setInputsReady] = useState(false);
  const openNewCollection = useCallback(() => setCollectionOpen(true), []);
  const openNewCollectionMobile = useCallback(() => {
    setMobileNav(false);
    setCollectionOpen(true);
  }, []);

  useEffect(() => {
    setInputsReady(true);
  }, []);

  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key === "/" && !(e.target instanceof HTMLInputElement) && !(e.target instanceof HTMLTextAreaElement)) {
        e.preventDefault();
        searchRef.current?.focus();
        return;
      }
      // In-window Escape only. Do not steal Escape from a text field, and do not
      // register a system-wide shortcut (that hid other apps' Escape).
      if (e.key === "Escape") {
        const t = e.target;
        if (
          t instanceof HTMLInputElement ||
          t instanceof HTMLTextAreaElement ||
          (t instanceof HTMLElement && t.isContentEditable)
        ) {
          return;
        }
        if (cancelDocsVfSyncFromShortcut()) {
          e.preventDefault();
        }
      }
    }
    window.addEventListener("keydown", onKey);
    const onGlobalContextMenu = (e: MouseEvent) => {
      if (e.defaultPrevented) return;
      const sel = window.getSelection?.()?.toString() ?? "";
      if (shouldAllowNativeContextMenu(e.target, sel)) return;
      e.preventDefault();
    };
    window.addEventListener("contextmenu", onGlobalContextMenu);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("contextmenu", onGlobalContextMenu);
    };
  }, []);

  return (
    <TooltipProvider delayDuration={220}>
      <div className="flex h-dvh flex-col overflow-hidden bg-background text-foreground">
        {/* 1.0.206ab: inert header during docs Cancel so FindFirst ≤300ms (Cancel is in shell-chrome below). */}
        <header
          className="fm-shell-header grid grid-cols-[auto_minmax(0,1fr)] items-center gap-x-1 gap-y-1 border-b border-border px-1 py-1 sm:grid-cols-[auto_minmax(0,1fr)_auto] sm:px-0 sm:py-0 md:grid-cols-[17rem_minmax(0,1fr)_auto]"
          inert={hideLibraryA11y || undefined}
        >
          <div className="flex items-center gap-1.5 pl-2 md:pl-3">
            <Button
              size="icon-sm"
              variant="ghost"
              className="md:hidden"
              aria-label="Open menu"
              onClick={() => setMobileNav(true)}
            >
              <Menu />
            </Button>
            <Link to="/" className="flex min-w-0 items-baseline gap-2 no-underline">
              <span className="truncate font-heading text-xl leading-none tracking-tight text-foreground">
                {APP_NAME}
              </span>
              <span
                className="font-mono text-[11px] tabular-nums text-muted-foreground"
                title={APP_BUILD_SHA ? `Build ${APP_BUILD_SHA}` : undefined}
              >
                {APP_VERSION}
                {APP_BUILD_SHA ? ` ${APP_BUILD_SHA.slice(0, 7)}` : ""}
              </span>
            </Link>
          </div>

          <div className="relative min-w-0">
            <Search className="pointer-events-none absolute left-2.5 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" />
            <HelpTip label="SuperSearch: quoted phrases, -negation, weight:>=700, axis:slnt, ot:smcp, source:local, category:sans, script:arabic. Press /">
              <Input
                ref={searchRef}
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                placeholder="Search name or SuperSearch"
                className="h-8 bg-card pl-8 pr-14"
                aria-label="Search typefaces"
              />
            </HelpTip>
            <div className="absolute right-1 top-1/2 flex -translate-y-1/2 items-center gap-0.5">
              {query ? (
                <button
                  type="button"
                  aria-label="Clear search"
                  data-testid="search-clear-btn"
                  onClick={() => {
                    setQuery("");
                    searchRef.current?.focus();
                  }}
                  className="inline-flex size-6 items-center justify-center rounded text-muted-foreground hover:bg-accent hover:text-foreground"
                >
                  <X className="size-3.5" />
                </button>
              ) : null}
              <Popover>
                <PopoverTrigger asChild>
                  <button
                    type="button"
                    aria-label="SuperSearch syntax"
                    data-testid="search-syntax-btn"
                    className="inline-flex size-6 items-center justify-center rounded text-muted-foreground hover:bg-accent hover:text-foreground"
                  >
                    <HelpCircle className="size-3.5" />
                  </button>
                </PopoverTrigger>
                <PopoverContent align="end" data-testid="search-syntax-popover" className="w-80 space-y-2.5 text-xs">
                  <div className="flex items-center justify-between gap-2">
                    <p className="font-medium text-foreground">SuperSearch syntax</p>
                    {query ? (
                      <button
                        type="button"
                        onClick={() => setQuery("")}
                        className="text-[11px] text-muted-foreground hover:text-foreground"
                      >
                        Reset
                      </button>
                    ) : null}
                  </div>
                  <p className="text-muted-foreground">
                    Combine free text, exact quotes, ranges, comparators (<code className="font-mono">&gt;= &lt;= +</code>), OpenType tags (<code className="font-mono">ot:</code> / <code className="font-mono">has:</code>), and <code className="font-mono">-</code> negation.
                  </p>
                  <div className="flex flex-wrap gap-1">
                    {[
                      '"Space Grotesk"',
                      "weight:>=700",
                      "axis:slnt",
                      "ot:smcp",
                      "has:liga",
                      "source:local",
                      "category:sans",
                      "category:icons",
                      "license:open",
                      "-italic",
                      "tag:geometric,grotesk",
                    ].map((token) => (
                      <button
                        key={token}
                        type="button"
                        onClick={() => setQuery(toggleSearchToken(query, token))}
                        className="rounded bg-secondary px-1.5 py-0.5 font-mono text-[11px] text-secondary-foreground hover:bg-accent"
                      >
                        {token}
                      </button>
                    ))}
                  </div>
                </PopoverContent>
              </Popover>
            </div>
          </div>

          <div className="hidden min-w-0 items-center gap-0.5 sm:flex">
            <nav className="mr-1.5 hidden items-center gap-0.5 lg:flex">
              {NAV.map((item) => (
                <Link
                  key={item.to}
                  to={item.to}
                  className={cn(
                    "inline-flex items-center gap-1 rounded-md px-2.5 py-1.5 text-sm no-underline transition-colors duration-150",
                    item.match(pathname)
                      ? "bg-accent text-foreground"
                      : "text-muted-foreground hover:text-foreground",
                  )}
                >
                  {item.label}
                  {item.beta ? (
                    <span className="rounded bg-secondary px-1 text-[9px] font-medium uppercase tracking-wide text-muted-foreground">
                      Beta
                    </span>
                  ) : null}
                </Link>
              ))}
            </nav>
            <ThemeToggle />
            <NotificationCenter />
            <DesktopSettings />
            <HelpTip label="Export CSS for activated fonts">
              <Button size="sm" variant="secondary" className="h-8 px-2.5" onClick={() => setCssOpen(true)}>
                <Code2 />
                CSS
              </Button>
            </HelpTip>
          </div>
          <div className="col-span-2 flex items-center justify-end gap-0.5 sm:hidden">
            <ThemeToggle />
            <NotificationCenter />
            <DesktopSettings />
            <HelpTip label="Export CSS">
              <Button size="icon-sm" variant="secondary" aria-label="Export CSS" onClick={() => setCssOpen(true)}>
                <Code2 />
              </Button>
            </HelpTip>
          </div>
        </header>
        {/* 1.0.206z: shell chrome (header already above + DownloadBar) stays outside library inert.
            data-fm-shell-chrome marks the Cancel host region for tip/structure asserts. */}
        {/* 1.0.206af: no role/aria-label on chrome host — WV2 FromPoint was landing on
            the region (empty/wrong Name) instead of Cancel Documents refresh. */}
        <div data-fm-shell-chrome="" className="shrink-0">
          <DownloadBar />
        </div>

        {/* 1.0.206y/aa/ab: inert library (+ header/nav) during docs sync so FindFirst Cancel ≤300ms.
            Never aria-hidden on large siblings (GetClickablePoint hang). Cancel stays in shell-chrome. */}
        <div
          data-fm-library-inert=""
          className="relative flex min-h-0 flex-1 overflow-hidden"
          inert={hideLibraryA11y || undefined}
        >
          <Sidebar
            onNewCollection={openNewCollection}
            className="hidden w-sidebar shrink-0 border-r border-border md:flex"
          />
          <main
            className="flex min-w-0 flex-1 flex-col overflow-hidden"
            aria-busy={hideLibraryA11y || undefined}
          >
            <TabPanes pathname={pathname} />
          </main>
          <FontInspector />
        </div>

        <nav
          className="grid grid-cols-4 border-t border-border bg-card lg:hidden"
          inert={hideLibraryA11y || undefined}
        >
          {NAV.map((item) => {
            const Icon = item.icon;
            const active = item.match(pathname);
            return (
              <Link
                key={item.to}
                to={item.to}
                className={cn(
                  "flex h-12 flex-col items-center justify-center gap-0.5 text-[11px] no-underline",
                  active ? "text-foreground" : "text-muted-foreground",
                )}
              >
                <Icon className="size-4" />
                <span className="inline-flex items-center gap-0.5">
                  {item.label}
                  {item.beta ? <span className="text-[8px] uppercase text-muted-foreground">β</span> : null}
                </span>
              </Link>
            );
          })}
        </nav>

        <Sheet open={mobileNav} onOpenChange={setMobileNav}>
          <SheetContent side="left" className="p-0">
            <div className="px-4 py-4 font-heading text-2xl">Library</div>
            <Sidebar
              onNewCollection={openNewCollectionMobile}
              className="h-[calc(100%-4rem)]"
            />
          </SheetContent>
        </Sheet>

        <ActivateConfirmDialog />
        <DeactivateConfirmDialog />
        <ImportOptionsDialog />
        <ImportCollisionDialog />
        <AppContextMenu />
        <CollectionDialog open={collectionOpen} onOpenChange={setCollectionOpen} />
        <CssExportDialog open={cssOpen} onOpenChange={setCssOpen} />

        {inputsReady && (
          <>
            <input
              ref={fileRef}
              data-testid="font-file-input"
              type="file"
              accept=".ttf,.otf,.woff,.woff2,.ttc,.otc"
              multiple
              className="hidden"
              onChange={(e) => {
                if (e.target.files) void runFontImport(e.target.files);
                e.target.value = "";
              }}
            />
            <input
              ref={(el) => {
                folderRef.current = el;
                if (!el) return;
                el.setAttribute("webkitdirectory", "");
                el.setAttribute("directory", "");
              }}
              data-testid="font-folder-input"
              type="file"
              multiple
              className="hidden"
              onChange={(e) => {
                if (e.target.files) void runFontImport(e.target.files);
                e.target.value = "";
              }}
            />
          </>
        )}
      </div>
    </TooltipProvider>
  );
}
