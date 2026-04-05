#!/usr/bin/env python3
import argparse
import json
import socket
import sys
import time


def send_cmd(host: str, port: int, payload: dict, timeout: float = 5.0) -> dict:
    with socket.create_connection((host, port), timeout=timeout) as s:
        s.sendall((json.dumps(payload) + "\n").encode("utf-8"))
        chunks = []
        while True:
            data = s.recv(4096)
            if not data:
                break
            chunks.append(data)
            if b"\n" in data:
                break
    raw = b"".join(chunks).split(b"\n", 1)[0].decode("utf-8", errors="replace")
    if not raw:
        return {"ok": False, "message": "empty response", "state": None}
    return json.loads(raw)


def state_to_map(state: dict):
    collection_sync_id = ""
    collection_deleted = None
    if state.get("collection"):
        collection_sync_id = state["collection"].get("sync_id") or ""
        collection_deleted = bool(state["collection"].get("deleted"))

    clips = {}
    for clip in state.get("clips", []):
        content = clip.get("content")
        if not content:
            continue
        clip_collection_sync_id = clip.get("collection_sync_id") or ""
        in_collection = bool(clip_collection_sync_id and clip_collection_sync_id == collection_sync_id)
        clips[content] = {
            "exists": clip.get("id") is not None,
            "pinned": bool(clip.get("pinned")) if clip.get("pinned") is not None else None,
            "in_collection": in_collection,
            "collection_sync_id": clip_collection_sync_id,
            "sync_version": clip.get("sync_version"),
            "source_device": clip.get("source_device"),
        }

    return {
        "collection_sync_id": collection_sync_id,
        "collection_deleted": collection_deleted,
        "clips": clips,
        "metadata_enabled": bool(state.get("metadata_enabled")),
        "sync_enabled": bool(state.get("sync_enabled")),
        "connected_peers": int(state.get("connected_peers") or 0),
        "device_id": state.get("device_id") or "",
    }


def expect_phase(phase: int):
    if phase == 1:
        return {
            "qa_pin_sync_clip_v1": {"pinned": True, "in_collection": True},
            "qa_pin_sync_clip_v2": {"pinned": False, "in_collection": True},
        }
    if phase == 2:
        return {
            "qa_pin_sync_clip_v1": {"pinned": False, "in_collection": False},
            "qa_pin_sync_clip_v2": {"pinned": True, "in_collection": True},
        }
    raise ValueError("invalid phase")


def wait_for_expected(host: str, port: int, phase: int, timeout_s: float) -> tuple[bool, dict, list[str]]:
    want = expect_phase(phase)
    start = time.time()
    last_map = {}
    errors = []

    while time.time() - start < timeout_s:
        resp = send_cmd(host, port, {"cmd": "state"})
        if not resp.get("ok"):
            errors = [f"state command failed: {resp.get('message')}"]
            time.sleep(0.25)
            continue
        last_map = state_to_map(resp.get("state") or {})

        local_errors = []
        if not last_map.get("sync_enabled"):
            local_errors.append("sync not enabled")
        if not last_map.get("metadata_enabled"):
            local_errors.append("metadata sync toggle disabled")
        for content, expected in want.items():
            clip = last_map.get("clips", {}).get(content)
            if not clip or not clip.get("exists"):
                local_errors.append(f"missing clip {content}")
                continue
            if clip.get("pinned") is not expected["pinned"]:
                local_errors.append(
                    f"{content} pinned={clip.get('pinned')} expected={expected['pinned']}"
                )
            if clip.get("in_collection") is not expected["in_collection"]:
                local_errors.append(
                    f"{content} in_collection={clip.get('in_collection')} expected={expected['in_collection']}"
                )

        if not local_errors:
            return True, last_map, []

        errors = local_errors
        time.sleep(0.25)

    return False, last_map, errors


def run_phase(apply_host: str, apply_port: int, verify_host: str, verify_port: int, phase: int, timeout_s: float) -> tuple[bool, dict, list[str], str]:
    apply_resp = send_cmd(apply_host, apply_port, {"cmd": "phase", "phase": phase})
    if not apply_resp.get("ok"):
        return False, {}, [f"apply phase {phase} failed: {apply_resp.get('message')}"], "apply"

    ok, state_map, errors = wait_for_expected(verify_host, verify_port, phase, timeout_s)
    if ok:
        return True, state_map, [], "verify"
    return False, state_map, errors, "verify"


def print_state(label: str, state_map: dict):
    print(f"[{label}] device={state_map.get('device_id')} sync_enabled={state_map.get('sync_enabled')} metadata_enabled={state_map.get('metadata_enabled')} connected_peers={state_map.get('connected_peers')}")
    print(f"[{label}] collection_sync_id={state_map.get('collection_sync_id')} deleted={state_map.get('collection_deleted')}")
    for content in ["qa_pin_sync_clip_v1", "qa_pin_sync_clip_v2"]:
        clip = state_map.get("clips", {}).get(content, {})
        print(
            f"[{label}] {content}: exists={clip.get('exists')} pinned={clip.get('pinned')} in_collection={clip.get('in_collection')}"
            f" sync_version={clip.get('sync_version')} collection_sync_id={clip.get('collection_sync_id')} source_device={clip.get('source_device')}"
        )


def ensure_seed_and_toggle(host: str, port: int):
    # force metadata toggle on in case config drifted
    resp = send_cmd(host, port, {"cmd": "set_metadata_sync", "enabled": True})
    if not resp.get("ok"):
        raise RuntimeError(f"set_metadata_sync failed on {host}:{port}: {resp.get('message')}")
    resp = send_cmd(host, port, {"cmd": "seed"})
    if not resp.get("ok"):
        raise RuntimeError(f"seed failed on {host}:{port}: {resp.get('message')}")


def main():
    parser = argparse.ArgumentParser(description="Rigorous cross-device metadata sync QA harness")
    parser.add_argument("--mac-port", type=int, default=51901)
    parser.add_argument("--win-port", type=int, default=51902)
    parser.add_argument("--timeout", type=float, default=25.0)
    parser.add_argument("--rounds", type=int, default=8)
    args = parser.parse_args()

    mac = ("127.0.0.1", args.mac_port)
    win = ("127.0.0.1", args.win_port)

    ensure_seed_and_toggle(*mac)
    ensure_seed_and_toggle(*win)

    # Warm-up state prints
    mac_state = send_cmd(*mac, {"cmd": "state"})
    win_state = send_cmd(*win, {"cmd": "state"})
    if not mac_state.get("ok") or not win_state.get("ok"):
        print("Failed to read initial state from one or both QA servers", file=sys.stderr)
        sys.exit(2)
    print_state("MAC:init", state_to_map(mac_state.get("state") or {}))
    print_state("WIN:init", state_to_map(win_state.get("state") or {}))

    failures = []

    for i in range(1, args.rounds + 1):
        phase = 1 if i % 2 == 1 else 2

        # mac -> win
        ok, state_map, errors, stage = run_phase(mac[0], mac[1], win[0], win[1], phase, args.timeout)
        if ok:
            print(f"ROUND {i} A mac->win phase={phase}: PASS")
        else:
            print(f"ROUND {i} A mac->win phase={phase}: FAIL ({stage})")
            print_state("WIN:fail", state_map)
            for e in errors:
                print(f"  - {e}")
            failures.append((i, "mac->win", phase, errors, state_map))

        # win -> mac
        ok, state_map, errors, stage = run_phase(win[0], win[1], mac[0], mac[1], phase, args.timeout)
        if ok:
            print(f"ROUND {i} B win->mac phase={phase}: PASS")
        else:
            print(f"ROUND {i} B win->mac phase={phase}: FAIL ({stage})")
            print_state("MAC:fail", state_map)
            for e in errors:
                print(f"  - {e}")
            failures.append((i, "win->mac", phase, errors, state_map))

    # collection deletion propagation check
    delete_resp = send_cmd(mac[0], mac[1], {"cmd": "delete_collection"})
    if not delete_resp.get("ok"):
        failures.append(("delete", "mac->win", None, [f"delete apply failed: {delete_resp.get('message')}"], {}))
    else:
        start = time.time()
        delete_ok = False
        last_map = {}
        delete_errors = []
        while time.time() - start < args.timeout:
            resp = send_cmd(win[0], win[1], {"cmd": "state"})
            if not resp.get("ok"):
                delete_errors = [f"state failed: {resp.get('message')}"]
                time.sleep(0.25)
                continue
            last_map = state_to_map(resp.get("state") or {})
            errs = []
            if last_map.get("collection_deleted") is not True:
                errs.append("collection not deleted on receiver")
            for content in ["qa_pin_sync_clip_v1", "qa_pin_sync_clip_v2"]:
                clip = last_map.get("clips", {}).get(content, {})
                if clip.get("in_collection"):
                    errs.append(f"{content} still in collection after delete")
            if not errs:
                delete_ok = True
                print("DELETE CHECK mac->win: PASS")
                break
            delete_errors = errs
            time.sleep(0.25)
        if not delete_ok:
            print("DELETE CHECK mac->win: FAIL")
            print_state("WIN:delete-fail", last_map)
            for e in delete_errors:
                print(f"  - {e}")
            failures.append(("delete", "mac->win", None, delete_errors, last_map))

    if failures:
        print(f"QA RESULT: FAIL ({len(failures)} failure(s))")
        sys.exit(1)

    print("QA RESULT: PASS")


if __name__ == "__main__":
    main()
