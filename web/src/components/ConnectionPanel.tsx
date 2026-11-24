import { useEffect, useState } from "react";
import type { FormEvent } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ChevronDown, LoaderCircle, X } from "lucide-react";
import { api, connectionSchema, connectionsSchema, date } from "../api";
import type { Connection, Connections } from "../api";
import { ErrorMessage } from "./Common";
import "../connection-panel.css";
import { retrySeconds, retryLabel } from "./connection-retry";

const pendingStates = new Set(["connecting", "awaiting_phone", "awaiting_code", "awaiting_password"]);
const names: Record<Connection["status"], string> = {
  unconfigured: "Setup needed", disconnected: "Disconnected", connecting: "Connecting…",
  connected: "Connected", awaiting_phone: "Sign in", awaiting_code: "Enter code",
  awaiting_password: "Enter password", error: "Needs attention",
};
const providerName = (provider: Connection["provider"]) => provider === "telegram" ? "Telegram" : "X";

export function useConnections() {
  return useQuery({
    queryKey: ["connections"],
    queryFn: ({ signal }) => api("/connections", connectionsSchema, { signal }),
    refetchInterval: query => query.state.data?.providers.some(provider => pendingStates.has(provider.status) || provider.retry_at !== null) ? 3000 : false,
    refetchIntervalInBackground: false,
  });
}

export function ConnectionPanel() {
  const connections = useConnections();
  const client = useQueryClient();
  const [selected, setSelected] = useState<Connection["provider"] | null>(null);
  const connection = connections.data?.providers.find(provider => provider.provider === selected);
  const [clock, setClock] = useState(Date.now);
  useEffect(() => {
    if (!connection?.retry_at) return;
    setClock(Date.now());
    const timer = window.setInterval(() => setClock(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [connection?.retry_at]);
  const waiting = retrySeconds(connection?.retry_at, clock);
  const update = (value: Connection) => {
    client.setQueryData<Connections>(["connections"], old => old ? { ...old, providers: old.providers.map(provider => provider.provider === value.provider ? value : provider) } : old);
    void client.invalidateQueries({ queryKey: ["connections"] });
    void client.invalidateQueries({ queryKey: ["subscriptions"] });
  };
  const change = useMutation({
    mutationFn: ({ provider, action }: { provider: Connection["provider"]; action: "connect" | "disconnect" }) => api(`/connections/${provider}/${action}`, connectionSchema, { method: "POST", body: {} }),
    onSuccess: update,
    onError: () => { void client.invalidateQueries({ queryKey: ["connections"] }); },
  });
  useEffect(() => { if (connection?.status === "connected") change.reset(); }, [connection?.provider, connection?.status]);

  return <section className="connections" aria-label="Connected accounts">
    <div className="connection-strip">
      {connections.isPending && <span className="connection-loading"><LoaderCircle size={13} className="spin"/>Checking connections…</span>}
      {connections.data?.providers.map(provider => <button key={provider.provider} className="connection-switch" aria-expanded={selected === provider.provider} aria-controls="connection-details" onClick={() => { setSelected(selected === provider.provider ? null : provider.provider); change.reset(); }}>
        <span className={`connection-dot ${provider.status}`} aria-hidden="true"/><strong>{providerName(provider.provider)}</strong><span>{names[provider.status]}</span><ChevronDown size={12}/>
      </button>)}
      <ErrorMessage error={connections.error}/>
    </div>
    {connections.data && !connections.data.collection_allowed && <p className="connection-workspace-state" role="status">{connections.data.workspace_detail}</p>}
    {connection && <div className="connection-details" id="connection-details">
      <div className="connection-detail-heading"><div><h3>{providerName(connection.provider)}{connection.account_label && <span>{connection.account_label}</span>}</h3><p>{connection.detail}</p></div><button className="icon-button" aria-label="Close connection details" onClick={() => setSelected(null)}><X size={15}/></button></div>
      {waiting > 0 && <p className="connection-retry">Retry after {date(connection.retry_at, true)}</p>}
      {connection.provider === "telegram" && ["awaiting_phone", "awaiting_code", "awaiting_password"].includes(connection.status) ? <TelegramAuth key={connection.status} connection={connection} waiting={waiting} onConnected={update}/> : null}
      <ErrorMessage error={change.error}/>
      <div className="connection-actions">
        {["unconfigured", "disconnected", "error"].includes(connection.status) ? <button className="button small" disabled={change.isPending || waiting > 0 || !connections.data?.collection_allowed || connection.status === "unconfigured"} onClick={() => change.mutate({ provider: connection.provider, action: "connect" })}>{change.isPending ? "Connecting…" : waiting > 0 ? retryLabel(waiting) : `Connect ${providerName(connection.provider)}`}</button> : <button className="text-button" disabled={change.isPending} onClick={() => change.mutate({ provider: connection.provider, action: "disconnect" })}>{change.isPending ? "Disconnecting…" : connection.status === "connected" ? "Disconnect" : "Cancel connection"}</button>}
      </div>
    </div>}
  </section>;
}

function TelegramAuth({ connection, waiting, onConnected }: { connection: Connection; waiting: number; onConnected: (connection: Connection) => void }) {
  const [value, setValue] = useState("");
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const field = connection.status === "awaiting_phone" ? "phone" : connection.status === "awaiting_code" ? "code" : "password";
  const title = field === "phone" ? "Phone number" : field === "code" ? "Telegram login code" : "Telegram two-step password";

  // Login values never enter draft storage, query keys, or mutation caches.
  // Clear the field before awaiting the request; unmounting clears local state.
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    const submitted = value;
    setValue(""); setError(null); setPending(true);
    try {
      onConnected(await api("/connections/telegram/auth", connectionSchema, { method: "POST", body: { [field]: submitted } }));
    } catch (error) { setError(error); }
    finally { setPending(false); }
  };
  return <form className="telegram-auth" onSubmit={submit} autoComplete="off">
    <label>{title}<input autoFocus autoComplete="off" type={field === "password" ? "password" : field === "phone" ? "tel" : "text"} inputMode={field === "code" ? "numeric" : undefined} required maxLength={field === "password" ? 256 : field === "phone" ? 32 : 16} value={value} onChange={event => setValue(event.target.value)} disabled={pending} placeholder={field === "phone" ? "+country code and number" : undefined}/></label>
    <button className="button primary" disabled={pending || waiting > 0 || !value.trim()}>{pending ? "Sending…" : waiting > 0 ? retryLabel(waiting) : field === "phone" ? "Send code" : "Continue"}</button>
    <ErrorMessage error={error}/>
  </form>;
}
