import { api, displayName, t, useSnapshot } from "./api";

export default function Banner() {
  const [snapshot] = useSnapshot();
  const session = snapshot?.session;
  if (!session) {
    return null;
  }

  return (
    <div
      data-tauri-drag-region
      className="flex h-full items-center justify-between gap-3 border-2 border-red-500 bg-white px-3 text-sm dark:bg-zinc-900"
    >
      <span data-tauri-drag-region className="flex items-center gap-2 truncate font-medium">
        <span className="h-2.5 w-2.5 shrink-0 animate-pulse rounded-full bg-red-500" />
        {t("controlling", { name: displayName(session) })}
      </span>
      <button
        className="shrink-0 rounded-md bg-red-600 px-3 py-1 text-xs font-medium text-white hover:bg-red-700"
        onClick={() => api.disconnect()}
      >
        {t("disconnect")}
      </button>
    </div>
  );
}
