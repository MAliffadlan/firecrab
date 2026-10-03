import type { VmReconciliationOutcome } from "./VmReconciliationOutcome";

/** Snapshot from the latest API startup; cleared on a new VM start. */
export type VmReconciliation = {
  outcome: VmReconciliationOutcome;
  checkedAtMs: number;
  detail: string | null;
};
