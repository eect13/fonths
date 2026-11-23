import { createFileRoute } from "@tanstack/react-router";
import { SkiaStudio } from "@/components/font-studio/skia-studio";

export const Route = createFileRoute("/skia")({ component: () => null });

export function SkiaPage() {
  return <SkiaStudio />;
}
