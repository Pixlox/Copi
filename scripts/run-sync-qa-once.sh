#!/usr/bin/env bash
set -euo pipefail

ROOT="/Users/omarsmac/Developer/Repos/Rust/copi"
REMOTE_HOST="omarp@192.168.1.153"
REMOTE_SRC="C:\\Users\\omarp\\Developer\\copi-lan\\Copi\\src-tauri"
SSH_OPTS=(
  -o ConnectTimeout=8
  -o PreferredAuthentications=password
  -o PubkeyAuthentication=no
  -o StrictHostKeyChecking=no
  -o UserKnownHostsFile=/dev/null
)

LOCAL_LOG="/tmp/copi_sync_qa_local_backend.log"
REMOTE_LOG="/tmp/copi_sync_qa_remote_backend.log"
TUNNEL_LOG="/tmp/copi_sync_qa_tunnel.log"

LOCAL_PID=""
REMOTE_SSH_PID=""
TUNNEL_PID=""

cleanup() {
  set +e

  if [[ -n "$LOCAL_PID" ]]; then
    kill "$LOCAL_PID" >/dev/null 2>&1 || true
  fi

  if [[ -n "$REMOTE_SSH_PID" ]]; then
    kill "$REMOTE_SSH_PID" >/dev/null 2>&1 || true
  fi

  if [[ -n "$TUNNEL_PID" ]]; then
    kill "$TUNNEL_PID" >/dev/null 2>&1 || true
  fi

  if [[ -f "/tmp/qa_local_tauri.pid" ]]; then
    LOCAL_DEV_PID_RAW="$(cat /tmp/qa_local_tauri.pid 2>/dev/null || true)"
    if [[ -n "$LOCAL_DEV_PID_RAW" ]]; then
      kill "$LOCAL_DEV_PID_RAW" >/dev/null 2>&1 || true
    fi
  fi

  sshpass -f ~/.sshpass ssh "${SSH_OPTS[@]}" "$REMOTE_HOST" "cmd /c \"taskkill /IM copi.exe /F >NUL 2>NUL\"" >/dev/null 2>&1 || true
  sshpass -f ~/.sshpass ssh "${SSH_OPTS[@]}" "$REMOTE_HOST" "python C:\\Users\\omarp\\remote_kill_all_tauri.py" >/dev/null 2>&1 || true
}
trap cleanup EXIT

echo "[QA] Clearing stale remote process"
sshpass -f ~/.sshpass ssh "${SSH_OPTS[@]}" "$REMOTE_HOST" "cmd /c \"taskkill /IM copi.exe /F >NUL 2>NUL\"" >/dev/null 2>&1 || true
sshpass -f ~/.sshpass ssh "${SSH_OPTS[@]}" "$REMOTE_HOST" "python C:\\Users\\omarp\\remote_kill_all_tauri.py" >/dev/null 2>&1 || true

echo "[QA] Starting local backend (QA port 51901)"
COPI_SYNC_QA_PORT=51901 cargo run --no-default-features --manifest-path "$ROOT/src-tauri/Cargo.toml" >"$LOCAL_LOG" 2>&1 &
LOCAL_PID="$!"

echo "[QA] Starting remote backend (QA port 51902)"
sshpass -f ~/.sshpass ssh "${SSH_OPTS[@]}" "$REMOTE_HOST" "cmd /c \"cd /d $REMOTE_SRC && set COPI_SYNC_QA_PORT=51902 && cargo run --no-default-features --\"" >"$REMOTE_LOG" 2>&1 &
REMOTE_SSH_PID="$!"

sleep 1

echo "[QA] Starting SSH tunnel 51902 -> remote localhost:51902"
sshpass -f ~/.sshpass ssh "${SSH_OPTS[@]}" -N -L 51902:127.0.0.1:51902 "$REMOTE_HOST" >"$TUNNEL_LOG" 2>&1 &
TUNNEL_PID="$!"

echo "[QA] Waiting for QA servers"
python3 - <<'PY'
import json
import socket
import time

deadline = time.time() + 180
ports = [51901, 51902]

def ping(port):
    try:
        with socket.create_connection(("127.0.0.1", port), timeout=1.2) as s:
            s.sendall((json.dumps({"cmd": "ping"}) + "\n").encode("utf-8"))
            data = s.recv(4096)
        if not data:
            return False
        payload = json.loads(data.decode("utf-8", errors="replace").strip())
        return bool(payload.get("ok"))
    except Exception:
        return False

while time.time() < deadline:
    status = {port: ping(port) for port in ports}
    if all(status.values()):
        print("QA_SERVERS_READY", status)
        raise SystemExit(0)
    time.sleep(1)

print("QA_SERVERS_TIMEOUT")
raise SystemExit(2)
PY

echo "[QA] Running rigorous metadata sync harness"
python3 "$ROOT/scripts/sync-metadata-qa.py" --rounds 20 --timeout 25

echo "[QA] PASS"
