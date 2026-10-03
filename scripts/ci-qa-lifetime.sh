#!/usr/bin/env bash
# QA VM lifetime (public-docs/qa.md R1–R7, X7): each VM's shim in its own
# systemd unit, a stop from outside the API, a quick restart, an API restart, a
# crash while the API is down, an interrupted start, and unit cleanup.
#
# Usage: scripts/ci-qa-lifetime.sh [OCI reference]   (default alpine:3.21)
#
# The reference's image is imported when its alias is not installed yet, and
# deleted again afterwards (X5); an alias that was already installed is kept.
#
# Root commands on the API host run through sudo on Linux, or as root on the
# management VM over SSH when FIRECRAB_QA_MANAGER_HOST and
# FIRECRAB_QA_MANAGER_KEY are set (macOS; ci-qa-macos-e2e.sh sets them).
set -euo pipefail

API=${FIRECRAB_API:-http://127.0.0.1:5523}
API=${API%/}
REFERENCE=${1:-alpine:3.21}
WAIT_FACTOR=${FIRECRAB_QA_WAIT_FACTOR:-1}
SUBNET=${FIRECRAB_QA_LIFETIME_SUBNET:-172.219.0.0/24}

pass() { printf 'PASS %s\n' "$1"; }
warning() { printf 'WARNING %s: %s\n' "$1" "$2"; }
fail() {
    printf 'FAILED %s: %s\n' "$1" "$2" >&2
    exit 1
}

# Runs one root shell command on the API host and prints its output.
host() {
    if [ -n "${FIRECRAB_QA_MANAGER_HOST:-}" ]; then
        local key=${FIRECRAB_QA_MANAGER_KEY:?FIRECRAB_QA_MANAGER_KEY is required with FIRECRAB_QA_MANAGER_HOST}
        ssh -i "$key" \
            -o BatchMode=yes \
            -o ConnectTimeout=10 \
            -o StrictHostKeyChecking=accept-new \
            -o "UserKnownHostsFile=$(dirname -- "$key")/known_hosts" \
            "root@${FIRECRAB_QA_MANAGER_HOST}" "$1" </dev/null
    else
        sudo sh -c "$1" </dev/null
    fi
}

api_ready() {
    for _ in $(seq 1 $((60 * WAIT_FACTOR))); do
        curl -fsS --max-time 5 "$API/api/host" >/dev/null 2>&1 && return 0
        sleep 1
    done
    return 1
}

vm_state() {
    curl -fsS --max-time 15 "$API/api/vms/$VM" | python3 -c 'import json,sys; print(json.load(sys.stdin)["state"])'
}

# Waits until the VM leaves `starting`/`stopping` and prints the state.
settled_state() {
    local state=
    for _ in $(seq 1 $((120 * WAIT_FACTOR))); do
        state=$(vm_state 2>/dev/null || true)
        case "$state" in
            starting | stopping | '') sleep 2 ;;
            *) break ;;
        esac
    done
    printf '%s\n' "$state"
}

start_vm() {
    curl -fsS -o /dev/null --max-time 300 -X POST "$API/api/vms/$VM/start"
}

shim_pid() { host "pgrep -f '[v]m-shim --vm-id $VM' || true"; }
vmm_pid() { host "pgrep -f '[f]irecracker --api-sock .*/$SIMPLE/' || true"; }
runtime_dir() {
    host "tr '\\0' '\\n' < /proc/$1/cmdline | sed -n '/^--runtime-dir\$/{n;p;}'"
}

json_field() {
    python3 -c 'import json,sys; d=json.load(sys.stdin); v=d.get(sys.argv[1]); print("" if v is None else str(v).lower() if isinstance(v, bool) else v)' "$1"
}

# Prints the alias for REFERENCE, importing it first when it is not installed.
installed_alias() {
    local alias status
    alias=$(curl -fsS --max-time 60 -G "$API/api/oci/inspect" --data-urlencode "reference=$REFERENCE" | json_field alias)
    [ -n "$alias" ] || fail R1 "no alias for $REFERENCE"
    if [ "$(curl -sS --max-time 30 "$API/api/images/$alias" | json_field installed 2>/dev/null)" != true ]; then
        curl -fsS -o /dev/null --max-time 60 -X POST "$API/api/oci/import" \
            -H 'content-type: application/json' \
            -d "$(python3 -c 'import json,sys; print(json.dumps({"reference": sys.argv[1]}))' "$REFERENCE")"
        IMPORTED=$alias
        for _ in $(seq 1 240); do
            status=$(curl -fsS --max-time 30 "$API/api/oci/import/$alias" | json_field status)
            case "$status" in
                succeeded) break ;;
                failed) fail R1 "importing $REFERENCE failed" ;;
                *) sleep 5 ;;
            esac
        done
        [ "$status" = succeeded ] || fail R1 "timed out importing $REFERENCE"
    fi
    printf '%s\n' "$alias"
}

NET=
VM=
IMPORTED=
cleanup() {
    if [ -n "${VM:-}" ]; then
        curl -sS -o /dev/null --max-time 120 -X POST "$API/api/vms/$VM/stop" || true
        curl -sS -o /dev/null --max-time 30 -X DELETE "$API/api/vms/$VM" || true
    fi
    if [ -n "${NET:-}" ]; then
        curl -sS -o /dev/null --max-time 15 -X DELETE "$API/api/micro-networks/$NET" || true
    fi
    if [ -n "${IMPORTED:-}" ]; then
        curl -sS -o /dev/null --max-time 60 -X DELETE "$API/api/images/$IMPORTED" || true
    fi
}
trap cleanup EXIT

TEMPLATE=$(installed_alias)
printf 'VM lifetime: template=%s api=%s\n' "$TEMPLATE" "$API"

NET=$(curl -fsS -X POST "$API/api/micro-networks" \
    -H 'content-type: application/json' \
    -d "{\"name\":\"qa-lifetime-$$\",\"subnetCidr\":\"$SUBNET\",\"internetEnabled\":true}" |
    python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')
VM=$(curl -fsS -X POST "$API/api/vms" \
    -H 'content-type: application/json' \
    -d "{\"name\":\"qa-lifetime-$$\",\"template\":\"$TEMPLATE\",\"cpu\":1,\"ram\":${FIRECRAB_QA_RAM:-512},\"diskGb\":${FIRECRAB_QA_DISK_GB:-2},\"microNetworkId\":\"$NET\"}" |
    python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')
SIMPLE=$(printf '%s' "$VM" | tr -d -)
TAP=$(python3 -c 'import hashlib, sys, uuid; print("fct" + hashlib.sha256(uuid.UUID(sys.argv[1]).bytes).hexdigest()[:12])' "$VM")
UNIT="firecrab-vm-$SIMPLE.service"

start_vm
[ "$(settled_state)" = running ] || fail R1 "VM did not reach running"

# R1: Firecracker runs under this VM's shim, in the VM's own unit, owned by
# PID 1 and run as the API user.
SHIM=$(shim_pid)
VMM=$(vmm_pid)
[ -n "$SHIM" ] || fail R1 "no shim for $VM"
[ -n "$VMM" ] || fail R1 "no Firecracker for $VM"
[ "$(host "ps -o ppid= -p $VMM" | tr -d ' ')" = "$SHIM" ] || fail R1 "Firecracker $VMM is not a child of shim $SHIM"
RUNTIME=$(runtime_dir "$SHIM")
host "test -S '$RUNTIME/shim.sock' && test -f '$RUNTIME/console.log'" || fail R1 "runtime $RUNTIME lacks shim.sock or console.log"
[ "$(host "systemctl is-active $UNIT" || true)" = active ] || fail R1 "$UNIT is not active"
[ "$(host "ps -o ppid= -p $SHIM" | tr -d ' ')" = 1 ] || fail R1 "shim $SHIM is not a child of PID 1"
api_user=$(host "systemctl show -p User --value firecrab-api")
[ "$(host "ps -o user= -p $SHIM" | tr -d ' ')" = "${api_user:-root}" ] || fail R1 "shim does not run as ${api_user:-root}"
pass "R1 shim $SHIM owns Firecracker $VMM in $UNIT, parent PID 1, user ${api_user:-root}"

# R2: a stop from outside the API (`systemctl stop`, as at host shutdown)
# records stopped, whatever signal ended Firecracker.
host "systemctl stop $UNIT"
[ "$(settled_state)" = stopped ] || fail R2 "VM is $(vm_state) after systemctl stop, expected stopped"
host "grep -q '\"stop_requested\":true' '$RUNTIME/exit.json'" || fail R2 "exit.json does not record the requested stop"
host "! ip link show $TAP >/dev/null 2>&1" || fail R2 "$TAP is still present"
pass "R2 systemctl stop recorded stopped and removed $TAP"

# R3: a start right after a stop reuses the unit name without waiting for the
# previous unit to be collected.
start_vm
[ "$(settled_state)" = running ] || fail R3 "VM did not reach running again"
curl -fsS -o /dev/null --max-time 120 -X POST "$API/api/vms/$VM/stop"
start_vm
[ "$(settled_state)" = running ] || fail R3 "VM is $(vm_state) after a stop and an immediate start, expected running"
SHIM=$(shim_pid)
VMM=$(vmm_pid)
pass "R3 a start right after a stop reached running"

# R4: an API restart re-adopts the running VM.
host "systemctl restart firecrab-api"
api_ready || fail R4 "API did not come back"
[ "$(shim_pid)" = "$SHIM" ] || fail R4 "the shim PID changed across the restart"
[ "$(vmm_pid)" = "$VMM" ] || fail R4 "the Firecracker PID changed across the restart"
[ "$(settled_state)" = running ] || fail R4 "VM is $(vm_state) after the restart, expected running"
host "journalctl -u firecrab-api -b --no-pager | grep 'startup reconciliation finished' | tail -1" |
    grep -Eq 'adopted=[1-9]' || fail R4 "the last reconciliation adopted no VM"
host "ip -o link show $TAP | grep -q ' master '" || fail R4 "$TAP is not attached to a bridge"
if command -v firecrab >/dev/null 2>&1; then
    # Ctrl+] detaches; reaching it proves the console WebSocket attached.
    printf '\035' | firecrab --api "$API" vm console "$VM" >/dev/null 2>&1 || fail R4 "console did not attach after adoption"
    pass "R4 API restart kept shim $SHIM, re-adopted the VM, kept $TAP, and console works"
else
    pass "R4 API restart kept shim $SHIM, re-adopted the VM, and kept $TAP"
    warning R4 "firecrab CLI not on PATH; console after adoption not checked"
fi

# R5: a crash while the API is down is recorded from exit.json.
host "systemctl stop firecrab-api; kill -9 $VMM; sleep 2; systemctl start firecrab-api"
api_ready || fail R5 "API did not come back"
[ "$(settled_state)" = error ] || fail R5 "VM is $(vm_state) after a crash, expected error"
host "! ip link show $TAP >/dev/null 2>&1" || fail R5 "$TAP is still present"
host "! nft list ruleset 2>/dev/null | grep -q '${VM%%-*}'" || fail R5 "nft rules for $VM are still present"
pass "R5 crash while the API was down recorded error and removed $TAP"

# R6: a start the restart cuts short is killed and recorded as an error.
start_vm
host "for i in \$(seq 1 200); do pgrep -f '[v]m-shim --vm-id $VM' >/dev/null && break; sleep 0.05; done; systemctl restart firecrab-api"
api_ready || fail R6 "API did not come back"
state=$(settled_state)
if [ "$state" = running ]; then
    warning R6 "the VM reached running before the restart; the interrupted start was not exercised"
else
    [ "$state" = error ] || fail R6 "VM is $state after an interrupted start, expected error"
    sleep 2
    [ "$(host "systemctl is-active $UNIT" || true)" != active ] || fail R6 "$UNIT still active"
    pass "R6 interrupted start recorded error and left no unit"
fi

# R7: a normal stop records stopped and leaves no failed unit.
if [ "$(vm_state)" != running ]; then
    start_vm
    [ "$(settled_state)" = running ] || fail R7 "VM did not reach running"
fi
RUNTIME=$(runtime_dir "$(shim_pid)")
curl -fsS -o /dev/null --max-time 120 -X POST "$API/api/vms/$VM/stop"
[ "$(settled_state)" = stopped ] || fail R7 "VM is $(vm_state) after stop, expected stopped"
host "grep -q '\"stop_requested\":true' '$RUNTIME/exit.json'" || fail R7 "exit.json does not record the requested stop"
host "! systemctl --failed --plain --no-legend | grep -q '$UNIT'" || fail R7 "$UNIT is failed"
pass "R7 stop recorded stopped and left no failed unit"

curl -fsS -o /dev/null --max-time 30 -X DELETE "$API/api/vms/$VM"
VM=
curl -fsS -o /dev/null --max-time 15 -X DELETE "$API/api/micro-networks/$NET"
NET=
[ -z "$(host "systemctl list-units --all --plain --no-legend '$UNIT'")" ] || fail X7 "$UNIT remains"
if [ -n "$IMPORTED" ]; then
    curl -fsS -o /dev/null --max-time 60 -X DELETE "$API/api/images/$IMPORTED"
    IMPORTED=
fi
pass "X7 no unit remains"
