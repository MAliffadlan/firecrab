import { useEffect, useRef, useState } from "react";
import { deleteFirecrabAddress, saveFirecrabAddress, type FirecrabAddress, type FirecrabTarget, type PipeSnapshot } from "../api/pipe";
import { useI18n } from "../i18n";
import PipeCopyButton from "./PipeCopyButton";
import PipeServiceMenu from "./PipeServiceMenu";

interface Props {
  snapshot: PipeSnapshot | null;
  busy: boolean;
  onBusy: (value: boolean) => void;
  onConnected: (snapshot: PipeSnapshot) => void;
  onUnsavedChange: (value: boolean) => void;
}
interface Editor { original: string | null; token: string; kind: "quick_tunnel" | "nginx"; source: string }

export default function FirecrabTunnel({ snapshot, busy, onBusy, onConnected, onUnsavedChange }: Props) {
  const { t } = useI18n();
  const [editor, setEditor] = useState<Editor | null>(null);
  const [error, setError] = useState<string | null>(null);
  const active = useRef(false);
  const addresses = snapshot?.firecrabTunnels ?? [];
  const quick = snapshot?.quickTunnels ?? [];
  const hosts = [...new Set(snapshot?.config?.routes.map(route => route.host) ?? [])];
  const changed = !!editor && (!!editor.token || !!editor.source);
  const sourceValid = !!editor && (editor.kind === "quick_tunnel" ? quick.some(q => q.id === editor.source) : hosts.includes(editor.source));
  const valid = !!editor && (!!editor.original || /^[a-fA-F0-9]{64}$/.test(editor.token.trim())) && sourceValid;
  useEffect(() => { onUnsavedChange(changed); return () => onUnsavedChange(false); }, [changed, onUnsavedChange]);

  async function mutate(request: () => Promise<PipeSnapshot>) {
    if (active.current || busy) return;
    active.current = true; onBusy(true); setError(null);
    try { onConnected(await request()); setEditor(null); }
    catch (failure) { setError((failure as Error).message); }
    finally { active.current = false; onBusy(false); }
  }
  function edit(address: FirecrabAddress) {
    setEditor({ original: address.subdomain, token: "", kind: address.target.type, source: address.target.type === "quick_tunnel" ? address.target.id : address.target.host });
    setError(null);
  }
  function target(): FirecrabTarget {
    return editor!.kind === "quick_tunnel" ? { type: "quick_tunnel", id: editor!.source } : { type: "nginx", host: editor!.source };
  }
  return <section className="panel" aria-label="Firecrab Tunnel">
    <h2 className="panel-title"><span className="panel-title-with-icon"><img className="panel-brand-icon" src="/firecrab.png" alt="" width={28} height={28} />Firecrab Tunnel</span><button className="btn" type="button" disabled={busy || !!editor || !snapshot || addresses.length >= 16} aria-expanded={!!editor} aria-controls="firecrab-address-editor" onClick={() => { setEditor({ original: null, token: "", kind: "quick_tunnel", source: "" }); setError(null); }}>{t("Add fixed address", "고정 주소 추가")}</button></h2>
    <p className="pipe-description">{t("Use a dashboard token to connect an existing Quick Tunnel or saved Nginx routes to your fixed address.", "대시보드에서 발급한 토큰으로 기존 Quick Tunnel 또는 저장된 Nginx 라우트를 고정 주소에 연결한다.")}</p>
    <p className="pipe-description"><a href="https://firecrab.dev/dashboard/" target="_blank" rel="noreferrer">{t("Get a token from the dashboard", "대시보드에서 토큰 발급")}</a></p>
    {error && <div className="banner error" role="alert">{error}</div>}
    {editor && <form id="firecrab-address-editor" className="create-grid quick-tunnel-form" onSubmit={event => { event.preventDefault(); if (valid) void mutate(() => saveFirecrabAddress(editor.original ? { subdomain: editor.original } : { token: editor.token.trim() }, target())); }}>
      {!editor.original ? <label className="field"><span>{t("Connection token", "연결 토큰")}</span><input type="password" aria-label={t("Connection token", "연결 토큰")} value={editor.token} maxLength={64} required autoComplete="off" spellCheck={false} disabled={busy} onChange={event => setEditor({ ...editor, token: event.target.value })} autoFocus /><span className="poll-note">{t("Paste the token issued by the Firecrab dashboard.", "Firecrab 대시보드에서 발급한 토큰을 붙여 넣는다.")}</span></label>
        : <p className="poll-note">{t("Uses the token already saved on this host.", "이 호스트에 저장한 토큰을 사용한다.")}</p>}
      <label className="field"><span>{t("Connection target", "연결 대상")}</span><select aria-label={t("Connection target", "연결 대상")} value={editor.kind === "nginx" ? "nginx" : editor.source} required disabled={busy} onChange={event => setEditor({ ...editor, kind: event.target.value === "nginx" ? "nginx" : "quick_tunnel", source: event.target.value === "nginx" ? "" : event.target.value })}>
        <option value="" disabled>{t("Select a target", "대상 선택")}</option>
        {!quick.length && <option value="quick_tunnel" disabled>Quick Tunnel</option>}
        {quick.map(q => <option key={q.id} value={q.id}>{q.name === "Quick Tunnel" ? "Quick Tunnel" : "Quick Tunnel · " + q.name}</option>)}
        <option value="nginx">{t("Nginx routes", "Nginx 라우트")}</option>
      </select>{!quick.length && editor.kind === "quick_tunnel" && <span className="poll-note">{t("No Quick Tunnels. Create one in Cloudflare Quick Tunnel, or select Nginx routes.", "Quick Tunnel이 없다. Cloudflare Quick Tunnel에서 생성하거나 Nginx 라우트를 선택한다.")}</span>}</label>
      {editor.kind === "nginx" && <label className="field"><span>{t("Nginx route domain", "Nginx 라우트 도메인")}</span><select aria-label={t("Nginx route domain", "Nginx 라우트 도메인")} value={editor.source} disabled={busy || !hosts.length} required onChange={event => setEditor({ ...editor, source: event.target.value })}><option value="">{t("Select saved routes", "저장된 라우트 선택")}</option>{hosts.map(host => <option key={host} value={host}>{host}</option>)}</select><span className="poll-note">{t("Publishes every saved path. The private Nginx connection is managed automatically.", "저장된 모든 경로를 공개한다. 사설 Nginx의 연결 프로그램은 자동으로 관리한다.")}</span></label>}
      <div className="pipe-controls quick-tunnel-submit"><button className="btn primary" type="submit" disabled={busy || !valid}>{busy ? t("Connecting…", "연결 중…") : t("Connect address", "주소 연결")}</button><button className="btn" type="button" disabled={busy} onClick={() => setEditor(null)}>{t("Cancel", "취소")}</button></div>
    </form>}
    {!snapshot ? <p className="poll-note">{t("Loading addresses…", "주소를 불러오는 중이다.")}</p> : !addresses.length ? <div className="empty">{t("No fixed addresses. Add a connection token and choose its target.", "고정 주소가 없다. 연결 토큰을 입력하고 대상을 선택한다.")}</div> : <div className="table-scroll"><table className="vm-table quick-tunnel-table"><thead><tr><th>{t("Address", "주소")}</th><th>{t("Target", "연결 대상")}</th><th>{t("State", "상태")}</th><th className="actions">{t("Actions", "작업")}</th></tr></thead><tbody>{addresses.map(address => <tr key={address.subdomain}>
      <td><div className="pipe-url"><a href={address.publicUrl} target="_blank" rel="noreferrer">{address.publicUrl}</a><PipeCopyButton url={address.publicUrl} onError={setError} /></div></td>
      <td>{address.target.type === "nginx" ? <>{t("Nginx routes", "Nginx 라우트")}<div className="quick-tunnel-target">{address.target.host}</div></> : <>Quick Tunnel<div className="quick-tunnel-target">{quick.find(q => q.id === (address.target.type === "quick_tunnel" ? address.target.id : ""))?.name ?? t("Unavailable", "확인 불가")}</div></>}</td>
      <td><span className={"state-badge" + (address.connected ? " running" : "")}>{address.connected ? t("Connected", "연결됨") : t("Pending", "연결 대기")}</span>{address.error && <div className="field-error">{address.error}</div>}</td>
      <td className="actions"><PipeServiceMenu label={address.subdomain + " " + t("actions", "작업")} disabled={busy} items={[{ label: t("Edit", "수정"), disabled: !!editor, onSelect: () => edit(address) }, { label: t("Delete", "삭제"), danger: true, onSelect: () => void mutate(() => deleteFirecrabAddress(address.subdomain)) }]} /></td>
    </tr>)}</tbody></table></div>}
  </section>;
}
