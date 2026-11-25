import { useMemo, useState } from "react";
import { Check, Copy } from "lucide-react";
import { toast } from "sonner";
import { copyText } from "@/lib/copy-text";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import {
  exportBundle,
  exportFontFamilies,
  exportGoogleImport,
  exportLinkTag,
  exportLocalFaces,
  exportTailwind,
} from "@/lib/fonts/css-export";
import { findFont, useFontStore } from "@/lib/fonts/store";

export function CssExportDialog({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const activated = useFontStore((s) => s.activated);
  const localFonts = useFontStore((s) => s.localFonts);
  const googleFonts = useFontStore((s) => s.googleFonts);
  const selectedId = useFontStore((s) => s.selectedId);
  const [copied, setCopied] = useState<string | null>(null);

  const { fonts, systemSkipped } = useMemo(() => {
    if (!open) return { fonts: [], systemSkipped: 0 };
    const ids = selectedId ? [selectedId, ...activated.filter((id) => id !== selectedId)] : activated;
    let skipped = 0;
    const list: NonNullable<ReturnType<typeof findFont>>[] = [];
    for (const id of ids) {
      if (id.startsWith("s:")) {
        skipped += 1;
        continue;
      }
      const found = findFont(id, localFonts, googleFonts);
      if (found && found.source !== "system") list.push(found);
      else if (found) skipped += 1;
    }
    return { fonts: list, systemSkipped: skipped };
  }, [open, activated, localFonts, googleFonts, selectedId]);

  const snippets = useMemo(() => {
    if (!open || !fonts.length) {
      return {
        bundle: "",
        import: "",
        link: "",
        families: "",
        faces: "",
        tailwind: "",
      };
    }
    return {
      bundle: exportBundle(fonts),
      import: exportGoogleImport(fonts) || "/* No Google Fonts families in the current set */",
      link: exportLinkTag(fonts) || "<!-- No Google Fonts families in the current set -->",
      families: exportFontFamilies(fonts),
      faces: exportLocalFaces(fonts) || "/* No uploaded files — catalog CSS does not need local @font-face */",
      tailwind: exportTailwind(fonts),
    };
  }, [open, fonts]);

  async function copy(key: string, value: string) {
    try {
      await copyText(value);
      setCopied(key);
      toast.success("Copied to clipboard");
      window.setTimeout(() => setCopied(null), 1400);
    } catch {
      toast.error("Clipboard blocked — select the CSS and press Ctrl+C");
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>CSS export</DialogTitle>
          <DialogDescription>
            {fonts.length
              ? `Activated typefaces${selectedId && !selectedId.startsWith("s:") ? ", with the open specimen first" : ""}.${
                  systemSkipped > 0 ? ` (${systemSkipped} system font${systemSkipped === 1 ? "" : "s"} excluded.)` : ""
                }`
              : systemSkipped > 0
                ? "System fonts are OS-installed and cannot be exported as @font-face or Google CSS."
                : "Activate a typeface to export CSS."}
          </DialogDescription>
        </DialogHeader>
        {fonts.length === 0 ? (
          <p className="text-sm text-muted-foreground">
            Toggle On on any card to include it here.
          </p>
        ) : (
          <Tabs defaultValue="bundle">
            <TabsList className="w-full justify-start overflow-x-auto">
              <TabsTrigger value="bundle">All</TabsTrigger>
              <TabsTrigger value="import">@import</TabsTrigger>
              <TabsTrigger value="link">Link</TabsTrigger>
              <TabsTrigger value="families">Stacks</TabsTrigger>
              <TabsTrigger value="faces">@font-face</TabsTrigger>
              <TabsTrigger value="tailwind">Theme</TabsTrigger>
            </TabsList>
            {Object.entries(snippets).map(([key, value]) => (
              <TabsContent key={key} value={key} className="relative">
                <pre className="max-h-64 overflow-auto rounded-lg bg-secondary p-3 font-mono text-xs leading-relaxed text-foreground">
                  {value}
                </pre>
                <Button
                  size="sm"
                  variant="secondary"
                  className="absolute right-2 top-2"
                  onClick={() => copy(key, value)}
                >
                  {copied === key ? <Check /> : <Copy />}
                  {copied === key ? "Copied" : "Copy"}
                </Button>
              </TabsContent>
            ))}
          </Tabs>
        )}
      </DialogContent>
    </Dialog>
  );
}
