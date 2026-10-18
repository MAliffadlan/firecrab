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
interface Editor { original: string | null; name: string; kind: "quick_tunnel" | "nginx"; source: string }

export default function FirecrabTunnel({ snapshot, busy, onBusy, onConnected, onUnsavedChange }: Props) {
  const { t } = useI18n();
  const [editor, setEditor] = useState<Editor | null>(null);
  const [error, setError] = useState<string | null>(null);
  const active = useRef(false);
  const addresses = snapshot?.firecrabTunnels ?? [];
  const quick = snapshot?.quickTunnels ?? [];
  const hosts = [...new Set(snapshot?.config?.routes.map(route => route.host) ?? [])];
  const changed = !!editor && (!!editor.name || !!editor.source);
  const sourceValid = !!editor && (editor.kind === "quick_tunnel" ? quick.some(q => q.id === editor.source) : hosts.includes(editor.source));
  const valid = !!editor && /^[a-z0-9][a-z0-9-]{1,46}[a-z0-9]$/.test(editor.name) && sourceValid;
  useEffect(() => { onUnsavedChange(changed); return () => onUnsavedChange(false); }, [changed, onUnsavedChange]);

  async function mutate(request: () => Promise<PipeSnapshot>) {
    if (active.current || busy) return;
    active.current = true; onBusy(true); setError(null);
    try { onConnected(await request()); setEditor(null); }
    catch (failure) { setError((failure as Error).message); }
    finally { active.current = false; onBusy(false); }
  }
  function edit(address: FirecrabAddress) {
    setEditor({ original: address.subdomain, name: address.subdomain, kind: address.target.type, source: address.target.type === "quick_tunnel" ? address.target.id : address.target.host });
    setError(null);
  }
  function target(): FirecrabTarget {
    return editor!.kind === "quick_tunnel" ? { type: "quick_tunnel", id: editor!.source } : { type: "nginx", host: editor!.source };
  }
  return <section className="panel" aria-label="Firecrab Tunnel">
    <h2 className="panel-title"><span>Firecrab Tunnel</span><button className="btn" type="button" disabled={busy || !!editor || !snapshot || addresses.length >= 16} aria-expanded={!!editor} aria-controls="firecrab-address-editor" onClick={() => { setEditor({ original: null, name: "", kind: "quick_tunnel", source: "" }); setError(null); }}>{t("Add fixed address", "고정 주소 추가")}</button></h2>
    <p className="pipe-description">{t("Connect a *.firecrab.dev address to an existing Quick Tunnel or saved Nginx routes. The address stays the same when the connector URL changes.", "*.firecrab.dev 주소를 기존 Quick Tunnel이나 저장된 Nginx 라우트에 연결한다. 연결 프로그램의 임시 주소가 바뀌어도 고정 주소를 유지한다.")}</p>
    {error && <div className="banner error" role="alert">{error}</div>}
    {editor && <form id="firecrab-address-editor" className="create-grid quick-tunnel-form" onSubmit={event => { event.preventDefault(); if (valid) void mutate(() => saveFirecrabAddress(editor.name, target())); }}>
      <label className="field"><span>{t("Subdomain", "서브도메인")}</span><div className="firecrab-domain-input"><input aria-label={t("Subdomain", "서브도메인")} value={editor.name} maxLength={48} required pattern="[a-z0-9][a-z0-9-]{1,46}[a-z0-9]" disabled={busy || !!editor.original} onChange={event => setEditor({ ...editor, name: event.target.value.toLowerCase() })} autoFocus /><span>.firecrab.dev</span></div></label>
      <label className="field"><span>{t("Connection target", "연결 대상")}</span><select aria-label={t("Connection target", "연결 대상")} value={editor.kind} disabled={busy} onChange={event => setEditor({ ...editor, kind: event.target.value as Editor["kind"], source: "" })}><option value="quick_tunnel">Quick Tunnel</option><option value="nginx">{t("Nginx routes", "Nginx 라우트")}</option></select></label>
      {editor.kind === "quick_tunnel" ? <label className="field"><span>Quick Tunnel</span><select aria-label="Quick Tunnel" value={editor.source} disabled={busy || !quick.length} required onChange={event => setEditor({ ...editor, source: event.target.value })}><option value="">{t("Select a tunnel", "터널 선택")}</option>{quick.map(q => <option key={q.id} value={q.id}>{q.name}</option>)}</select>{!quick.length && <span className="poll-note">{t("Create a Quick Tunnel first, or select Nginx routes.", "Quick Tunnel을 먼저 만들거나 Nginx 라우트를 선택한다.")}</span>}</label>
        : <label className="field"><span>{t("Nginx route domain", "Nginx 라우트 도메인")}</span><select aria-label={t("Nginx route domain", "Nginx 라우트 도메인")} value={editor.source} disabled={busy || !hosts.length} required onChange={event => setEditor({ ...editor, source: event.target.value })}><option value="">{t("Select saved routes", "저장된 라우트 선택")}</option>{hosts.map(host => <option key={host} value={host}>{host}</option>)}</select><span className="poll-note">{t("Publishes every saved path. The private Nginx connection is managed automatically.", "저장된 모든 경로를 공개한다. 사설 Nginx의 연결 프로그램은 자동으로 관리한다.")}</span></label>}
      <div className="pipe-controls quick-tunnel-submit"><button className="btn primary" type="submit" disabled={busy || !valid}>{busy ? t("Connecting…", "연결 중…") : t("Connect address", "주소 연결")}</button><button className="btn" type="button" disabled={busy} onClick={() => setEditor(null)}>{t("Cancel", "취소")}</button></div>
    </form>}
    {!snapshot ? <p className="poll-note">{t("Loading addresses…", "주소를 불러오는 중이다.")}</p> : !addresses.length ? <div className="empty">{t("No fixed addresses. Add an address and choose its target.", "고정 주소가 없다. 주소를 추가하고 연결 대상을 선택한다.")}</div> : <div className="table-scroll"><table className="vm-table quick-tunnel-table"><thead><tr><th>{t("Address", "주소")}</th><th>{t("Target", "연결 대상")}</th><th>{t("State", "상태")}</th><th className="actions">{t("Actions", "작업")}</th></tr></thead><tbody>{addresses.map(address => <tr key={address.subdomain}>
      <td><div className="pipe-url"><a href={address.publicUrl} target="_blank" rel="noreferrer">{address.publicUrl}</a><PipeCopyButton url={address.publicUrl} onError={setError} /></div></td>
      <td>{address.target.type === "nginx" ? <>{t("Nginx routes", "Nginx 라우트")}<div className="quick-tunnel-target">{address.target.host}</div></> : <>Quick Tunnel<div className="quick-tunnel-target">{quick.find(q => q.id === (address.target.type === "quick_tunnel" ? address.target.id : ""))?.name ?? t("Unavailable", "확인 불가")}</div></>}</td>
      <td><span className={"state-badge" + (address.connected ? " running" : "")}>{address.connected ? t("Connected", "연결됨") : t("Pending", "연결 대기")}</span>{address.error && <div className="field-error">{address.error}</div>}</td>
      <td className="actions"><PipeServiceMenu label={address.subdomain + " " + t("actions", "작업")} disabled={busy} items={[{ label: t("Edit", "수정"), disabled: !!editor, onSelect: () => edit(address) }, { label: t("Delete", "삭제"), danger: true, onSelect: () => void mutate(() => deleteFirecrabAddress(address.subdomain)) }]} /></td>
    </tr>)}</tbody></table></div>}
  </section>;
}
