import { useEffect, type ReactNode } from "react";
import { Toaster } from "sonner";
import { installNotificationRecorder } from "@/lib/notifications";
import { hydrateTheme, useTheme } from "@/lib/theme";
import { hydrateUiDensity } from "@/lib/ui-density";

installNotificationRecorder();

export function ThemeRoot({ children }: { children: ReactNode }) {
  const theme = useTheme();

  useEffect(() => {
    hydrateTheme();
    hydrateUiDensity();
  }, []);

  return (
    <>
      {children}
      <Toaster
        theme={theme}
        position="bottom-right"
        closeButton
        visibleToasts={3}
        duration={4500}
        expand={false}
        toastOptions={{
          style: {
            background: "var(--color-card)",
            color: "var(--color-foreground)",
            border: "1px solid var(--color-border)",
          },
        }}
      />
    </>
  );
}
