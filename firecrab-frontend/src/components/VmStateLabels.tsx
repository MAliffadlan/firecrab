import type { VmResponse, VmState } from "../bindings";
import { useI18n } from "../i18n";
import ReconciliationStatus from "./ReconciliationStatus";

const LABELS: Record<VmState, [string, string]> = {
  created: ["Created", "생성됨"],
  starting: ["Starting", "시작 중"],
  running: ["Running", "실행 중"],
  stopping: ["Stopping", "중지 중"],
  stopped: ["Stopped", "중지됨"],
  error: ["Error", "오류"],
};

export default function VmStateLabels({ vm }: { vm: VmResponse }) {
  const { t } = useI18n();
  const label = `VM-STATUS: ${t(...LABELS[vm.state])}`;
  return (
    <div className="vm-state-labels">
      <strong
        className={`state-label vm-status-label ${vm.state}`}
        role="status"
        aria-label={label}
        title={label}
        data-state={vm.state}
        tabIndex={0}
      >
        VM
      </strong>
      <ReconciliationStatus result={vm.reconciliation} />
    </div>
  );
}
