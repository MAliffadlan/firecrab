import { useEffect, useRef, useState, type FormEvent } from "react";
import { applyPipe, disablePipe, getPipe, type PipeConfig, type PipeRoute, type PipeSnapshot } from "../api/pipe";
import { useI18n } from "../i18n";

const EMPTY: PipeConfig = { listen: "127.0.0.1:8080", routes: [], tunnel: null };

export default function Pipe() {
  const { t } = useI18n();
  const [snapshot, setSnapshot] = useState<PipeSnapshot | null>(null);
  const [draft, setDraft] = useState<PipeConfig>(EMPTY);
  const [host, setHost] = useState("");
  const [path, setPath] = useState("/");
  const [upstreams, setUpstreams] = useState("");
  const [editing, setEditing] = useState<number | null>(null);
  const [credentials, setCredentials] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [dirty, setDirty] = useState(false);
  const [busy, setBusy] = useState(false);
  const initialized = useRef(false);
  const revision = useRef(0);

  useEffect(() => {
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let controller: AbortController | undefined;
    const refresh = async () => {
      const current = revision.current;
      controller = new AbortController();
      const deadline = setTimeout(() => controller?.abort(), 10_000);
      try {
        const next = await getPipe(controller.signal);
        if (cancelled || current !== revision.current) return;
        setSnapshot(next);
        setError(null);
        if (!initialized.current) { setDraft(next.config ?? EMPTY); initialized.current = true; }
      } catch (failure) {
        if (cancelled || current !== revision.current) return;
        setSnapshot(null);
        setError((failure as Error).message);
      } finally {
        clearTimeout(deadline);
        if (!cancelled) timer = setTimeout(refresh, 8_000);
      }
    };
    void refresh();
    return () => { cancelled = true; clearTimeout(timer); controller?.abort(); };
  }, []);

  function change(next: PipeConfig) { initialized.current = true; setDraft(next); setDirty(true); setNotice(null); }
  function clearEditor() { setHost(""); setPath("/"); setUpstreams(""); setEditing(null); }
  function edit(route: PipeRoute, index: number) { setHost(route.host); setPath(route.pathPrefix); setUpstreams(route.upstreams.join(", ")); setEditing(index); }
  function addRoute(event: FormEvent) {
    event.preventDefault();
    const route = { host: host.trim().toLowerCase(), pathPrefix: path.trim() || "/", upstreams: upstreams.split(/[\s,]+/).filter(Boolean) };
    const duplicate = draft.routes.some((existing, index) => index !== editing && existing.host === route.host && existing.pathPrefix.replace(/\/+$/, "") === route.pathPrefix.replace(/\/+$/, ""));
    if (duplicate) { setError(t("This domain and path already exist.", "이 도메인과 경로가 이미 있다.")); return; }
    change({ ...draft, routes: editing === null ? [...draft.routes, route] : draft.routes.map((existing, index) => index === editing ? route : existing) });
    clearEditor(); setError(null);
  }
  async function save() {
    if (busy) return;
    if (draft.tunnel && !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(draft.tunnel.id.trim())) { setError(t("Enter a valid tunnel UUID.", "올바른 터널 UUID를 입력한다.")); return; }
    let secret: unknown = null;
    try { if (draft.tunnel && credentials.trim()) secret = JSON.parse(credentials); }
    catch { setError(t("Tunnel credentials must be valid JSON.", "터널 인증 정보는 올바른 JSON이어야 한다.")); return; }
    setBusy(true); revision.current++; setError(null); setNotice(null);
    try {
      const next = await applyPipe(draft, secret);
      setSnapshot(next); setDraft(next.config ?? draft); setDirty(false); setCredentials("");
      setNotice(t("Routes applied. Existing requests continue during reload.", "라우트를 적용했다. 설정 전환 중 기존 요청을 계속 처리한다."));
    } catch (failure) { setError((failure as Error).message); }
    finally { setBusy(false); revision.current++; }
  }
  async function stop() {
    if (busy || !window.confirm(t("Stop Pipe? Requests through these routes will stop.", "Pipe를 중지할까요? 이 경로의 요청 전달이 중지된다."))) return;
    setBusy(true); revision.current++; setError(null); setNotice(null);
    try { setSnapshot(await disablePipe()); setNotice(t("Pipe stopped. Saved routes are retained.", "Pipe를 중지했다. 저장한 라우트는 유지한다.")); }
    catch (failure) { setError((failure as Error).message); }
    finally { setBusy(false); revision.current++; }
  }

  return <div className="stack">
    <section className="panel" aria-label="Pipe">
      <h2 className="panel-title"><span>Pipe</span><span className="poll-note">{t("Nginx routing and load balancing", "Nginx 요청 전달과 요청 분산")}</span></h2>
      {error && <div className="banner error" role="alert">{error}</div>}
      {notice && <div className="banner info" role="status">{notice}</div>}
      <div className="table-scroll"><table className="vm-table pipe-status-table"><thead><tr><th>{t("Service", "서비스")}</th><th>{t("State", "상태")}</th><th>PID</th></tr></thead><tbody>
        {["firecrab-ingress.service", "firecrab-tunnel.service"].map(unit => {
          const status = snapshot?.units.find(value => value.unit === unit);
          const state = status?.activeState ?? "unknown";
          return <tr key={unit}><td>{unit.startsWith("firecrab-ingress") ? "Nginx" : "Cloudflare Tunnel"}</td><td><span className={`state-badge${state === "active" ? " running" : state === "failed" ? " error" : ""}`}>{status ? state : t("Unavailable", "확인 불가")}</span></td><td>{status?.mainPid || "—"}</td></tr>;
        })}
      </tbody></table></div>
      {snapshot && !snapshot.nginxAvailable && <p className="field-error">{t("Install Nginx on this host before applying routes.", "라우트를 적용하기 전에 이 호스트에 Nginx를 설치한다.")}</p>}
      <div className="pipe-controls">
        <label className="field"><span>{t("Listen address", "수신 주소")}</span><input aria-label={t("Listen address", "수신 주소")} value={draft.listen} disabled={busy} onChange={event => change({ ...draft, listen: event.target.value })} /></label>
        <button className="btn primary" type="button" disabled={busy || !snapshot?.nginxAvailable || editing !== null || !!host.trim() || (draft.tunnel !== null && !snapshot.cloudflaredAvailable)} onClick={save}>{busy ? t("Applying…", "처리 중…") : t("Save and apply", "저장·적용")}</button>
        <button className="btn" type="button" disabled={busy || !snapshot || !snapshot.units.some(unit => unit.activeState === "active")} onClick={stop}>{t("Stop", "중지")}</button>
        <span className="poll-note">{dirty ? t("Unapplied changes", "적용하지 않은 변경") : t("Saved configuration", "저장된 설정")}</span>
      </div>
    </section>

    <section className="panel" aria-label={t("Routes", "라우트")}>
      <h2 className="panel-title">{t("Routes", "라우트")}</h2>
      <form className="create-grid pipe-route-form" onSubmit={addRoute}>
        <div className="field"><label htmlFor="pipe-domain">{t("Domain", "도메인")}</label><input id="pipe-domain" placeholder="app.example.com" value={host} onChange={event => setHost(event.target.value)} required disabled={busy} /></div>
        <div className="field"><label htmlFor="pipe-path">{t("Path prefix", "경로 시작")}</label><input id="pipe-path" placeholder="/api" value={path} onChange={event => setPath(event.target.value)} required pattern="/.*" disabled={busy} /></div>
        <div className="field pipe-targets"><label htmlFor="pipe-upstreams">{t("App addresses", "앱 주소")}</label><input id="pipe-upstreams" placeholder="127.0.0.1:18081, 127.0.0.1:18082" value={upstreams} onChange={event => setUpstreams(event.target.value)} required disabled={busy} /><span className="poll-note">{t("IP:port, separated by commas. Requests are distributed in turn.", "IP:포트를 쉼표로 구분한다. 요청을 순서대로 분산한다.")}</span></div>
        <div className="field field-submit"><label>&nbsp;</label><button className="btn" type="submit" disabled={busy}>{editing === null ? t("Add route", "라우트 추가") : t("Update route", "라우트 수정")}</button>{editing !== null && <button type="button" className="btn" onClick={clearEditor}>{t("Cancel", "취소")}</button>}</div>
      </form>
      <div className="table-scroll"><table className="vm-table pipe-route-table"><thead><tr><th>{t("Domain", "도메인")}</th><th>{t("Path", "경로")}</th><th>{t("App addresses", "앱 주소")}</th><th>{t("Actions", "작업")}</th></tr></thead><tbody>
        {draft.routes.map((route, index) => <tr key={`${index}:${route.host}:${route.pathPrefix}`}><td>{route.host}</td><td>{route.pathPrefix}</td><td className="pipe-addresses">{route.upstreams.map(address => <div key={address}>{address}</div>)}</td><td><div className="pipe-row-actions"><button className="btn" type="button" disabled={busy} onClick={() => edit(route, index)}>{t("Edit", "수정")}</button><button className="btn" type="button" disabled={busy} onClick={() => { change({ ...draft, routes: draft.routes.filter((_, row) => row !== index) }); clearEditor(); }}>{t("Remove", "제거")}</button></div></td></tr>)}
      </tbody></table>{!draft.routes.length && <div className="empty">{t("No routes. Add a domain, path and app addresses.", "라우트가 없다. 도메인, 경로, 앱 주소를 추가한다.")}</div>}</div>
    </section>

    <section className="panel" aria-label="Cloudflare Tunnel">
      <h2 className="panel-title">{t("Cloudflare Tunnel (optional)", "Cloudflare Tunnel (선택)")}</h2>
      <label className="checkbox-line"><input type="checkbox" checked={draft.tunnel !== null} disabled={busy} onChange={event => { setCredentials(""); change({ ...draft, tunnel: event.target.checked ? { id: "", credentialsFile: "credentials.json" } : null }); }} />{t("Enable public access through Cloudflare", "Cloudflare로 외부 접속 사용")}</label>
      {draft.tunnel && <div className="pipe-tunnel-fields">
        <div className="field"><label htmlFor="pipe-tunnel-id">Tunnel UUID</label><input id="pipe-tunnel-id" value={draft.tunnel.id} disabled={busy} onChange={event => change({ ...draft, tunnel: { ...draft.tunnel!, id: event.target.value.trim() } })} /></div>
        <div className="field"><label htmlFor="pipe-credentials">{t("Credentials JSON", "인증 정보 JSON")}</label><textarea id="pipe-credentials" className="pipe-credentials" autoComplete="off" spellCheck={false} value={credentials} disabled={busy} onChange={event => { setCredentials(event.target.value); setDirty(true); }} placeholder={t("Paste the tunnel credentials. Leave blank to keep saved credentials.", "터널 인증 정보를 입력한다. 비워 두면 저장한 인증 정보를 유지한다.")} /><span className="poll-note">{t("Credentials are cleared after saving and are never returned to the dashboard.", "저장 후 입력을 지운다. 저장한 인증 정보는 화면으로 반환하지 않는다.")}</span></div>
        {!snapshot?.cloudflaredAvailable && <p className="field-error">{t("Install the connector with firecrab pipe install.", "firecrab pipe install로 연결 프로그램을 설치한다.")}</p>}
      </div>}
    </section>
  </div>;
}
