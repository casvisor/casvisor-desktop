import { useEffect, useState } from "react";

import { api, displayName, SessionInfo, t, useSnapshot } from "./api";
import logo from "./logo.png";

const statusColor: Record<string, string> = {
  online: "bg-emerald-500",
  connecting: "bg-amber-400",
  offline: "bg-gray-400",
  error: "bg-red-500",
  unconfigured: "bg-gray-400",
};

function Consent({ session }: { session: SessionInfo }) {
  const [seconds, setSeconds] = useState(30);

  useEffect(() => {
    const timer = setInterval(() => setSeconds((s) => Math.max(0, s - 1)), 1000);
    return () => clearInterval(timer);
  }, [session.id]);

  return (
    <div className="fixed inset-0 z-10 flex items-center justify-center bg-black/40 p-4">
      <div className="w-full max-w-sm rounded-xl bg-white p-5 shadow-xl dark:bg-zinc-900">
        <h2 className="text-base font-semibold">{t("request", { name: displayName(session) })}</h2>
        <p className="mt-1 text-sm text-gray-500 dark:text-gray-400">
          {t("requestDetail", { user: session.user, ip: session.clientIp })}
        </p>
        <div className="mt-4 text-sm">
          <div className="text-gray-500 dark:text-gray-400">{t("permissions")}</div>
          <ul className="mt-1 list-inside list-disc">
            {session.permissions.map((p) => (
              <li key={p}>{t(p)}</li>
            ))}
          </ul>
        </div>
        <div className="mt-5 flex justify-end gap-2">
          <button
            className="rounded-md border border-gray-300 px-4 py-2 text-sm hover:bg-gray-50 dark:border-zinc-700 dark:hover:bg-zinc-800"
            onClick={() => api.answerConsent(session.id, false)}
          >
            {t("deny", { seconds })}
          </button>
          <button
            className="rounded-md bg-blue-600 px-4 py-2 text-sm font-medium text-white hover:bg-blue-700"
            onClick={() => api.answerConsent(session.id, true)}
          >
            {t("allow")}
          </button>
        </div>
      </div>
    </div>
  );
}

export default function App() {
  const [snapshot] = useSnapshot();
  const [server, setServer] = useState("");
  const [fingerprint, setFingerprint] = useState("");
  const [token, setToken] = useState("");
  const [unattended, setUnattended] = useState(false);
  const [message, setMessage] = useState("");
  const [loaded, setLoaded] = useState(false);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (snapshot && !loaded && (!snapshot.service || snapshot.config.server)) {
      setServer(snapshot.config.server);
      setFingerprint(snapshot.config.fingerprint);
      setUnattended(snapshot.config.unattended);
      setLoaded(true);
    }
  }, [snapshot, loaded]);

  const run = async (action: () => Promise<void>, done: string, asksAdmin: boolean) => {
    setMessage(asksAdmin ? t("working") : "");
    setBusy(true);
    try {
      await action();
      setMessage(done);
    } catch (e) {
      setMessage(String(e));
    } finally {
      setBusy(false);
    }
  };

  if (!snapshot) {
    return null;
  }

  const { status, session } = snapshot;
  const statusText = t(status.state, { server: status.server });

  const save = () =>
    run(
      async () => {
        await api.saveSettings({ server, fingerprint, token, unattended });
        setToken("");
      },
      t("saved"),
      snapshot.service,
    );

  const input =
    "mt-1 w-full rounded-md border border-gray-300 bg-white px-3 py-2 text-sm outline-none focus:border-blue-500 dark:border-zinc-700 dark:bg-zinc-950";

  return (
    <div className="mx-auto flex max-w-lg flex-col gap-4 p-5">
      <header className="flex items-center gap-3">
        <img src={logo} alt="" className="h-9 w-9" />
        <div>
          <div className="text-lg font-semibold">Casvisor Desktop</div>
          <div className="text-xs text-gray-500">v{snapshot.version}</div>
        </div>
      </header>

      <section className="rounded-xl bg-white p-4 shadow-sm dark:bg-zinc-900">
        <div className="flex items-center gap-2">
          <span className={`h-2.5 w-2.5 rounded-full ${statusColor[status.state] ?? "bg-gray-400"}`} />
          <span className="text-sm font-medium">{statusText}</span>
        </div>
        {status.message && <p className="mt-2 text-xs break-words text-gray-500">{status.message}</p>}
      </section>

      {session?.accepted && (
        <section className="flex items-center justify-between gap-3 rounded-xl border border-red-200 bg-red-50 p-4 dark:border-red-900 dark:bg-red-950/40">
          <span className="text-sm font-medium">{t("controlling", { name: displayName(session) })}</span>
          <button
            className="rounded-md bg-red-600 px-3 py-1.5 text-sm font-medium text-white hover:bg-red-700"
            onClick={() => api.disconnect()}
          >
            {t("disconnect")}
          </button>
        </section>
      )}

      <section className="rounded-xl bg-white p-4 shadow-sm dark:bg-zinc-900">
        <h2 className="text-sm font-semibold">{t("settings")}</h2>
        <label className="mt-3 block text-sm">
          {t("server")}
          <input className={input} value={server} placeholder={t("serverHint")} onChange={(e) => setServer(e.target.value)} />
        </label>
        <label className="mt-3 block text-sm">
          {t("token")}
          <input
            className={input}
            type="password"
            value={token}
            placeholder={snapshot.config.hasToken ? t("tokenSaved") : "cva_…"}
            onChange={(e) => setToken(e.target.value)}
          />
        </label>
        <label className="mt-3 block text-sm">
          {t("fingerprint")}
          <input className={input} value={fingerprint} placeholder="SHA256:…" onChange={(e) => setFingerprint(e.target.value)} />
        </label>
        <label className="mt-3 flex items-center gap-2 text-sm">
          <input type="checkbox" checked={unattended} onChange={(e) => setUnattended(e.target.checked)} />
          {t("unattended")}
        </label>
        <div className="mt-4 flex items-center gap-3">
          <button
            className="rounded-md bg-blue-600 px-4 py-2 text-sm font-medium text-white hover:bg-blue-700 disabled:opacity-50"
            disabled={busy}
            onClick={save}
          >
            {t("save")}
          </button>
          {message && <span className="text-xs break-words text-gray-500">{message}</span>}
        </div>
      </section>

      <section className="rounded-xl bg-white p-4 shadow-sm dark:bg-zinc-900">
        <h2 className="text-sm font-semibold">{t("service")}</h2>
        <p className="mt-2 text-xs text-gray-500">{t(snapshot.service ? "serviceOn" : "serviceOff")}</p>
        <button
          className="mt-3 rounded-md border border-gray-300 px-4 py-2 text-sm hover:bg-gray-50 disabled:opacity-50 dark:border-zinc-700 dark:hover:bg-zinc-800"
          disabled={busy}
          onClick={() => run(snapshot.service ? api.uninstallService : api.installService, "", true)}
        >
          {t(snapshot.service ? "uninstallService" : "installService")}
        </button>
      </section>

      {session && !session.accepted && <Consent session={session} />}
    </div>
  );
}
