#!/usr/bin/env python3
import argparse
import json
import socket
import sys
import time

QA_COLLECTION_SYNC_ID = "qa-sync-coll-v1"
QA_DELETE_RECOPY_TEXT = "qa_delete_recopy_clip_text_v1"


def send_cmd(host: str, port: int, payload: dict, timeout: float = 10.0) -> dict:
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


def wait_for_connected(host: str, port: int, timeout_s: float) -> None:
    start = time.time()
    last_err = "unknown"
    while time.time() - start < timeout_s:
        try:
            resp = send_cmd(host, port, {"cmd": "state"}, timeout=2.0)
            if resp.get("ok"):
                state = state_to_map(resp.get("state") or {})
                if state.get("connected_peers", 0) > 0:
                    return
                last_err = "connected_peers=0"
            else:
                last_err = f"state not ok: {resp.get('message')}"
        except Exception as exc:
            last_err = str(exc)
        time.sleep(0.25)
    raise RuntimeError(f"peer connection did not become active on {host}:{port} ({last_err})")


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
            "exists": clip.get("id") is not None and not bool(clip.get("deleted")),
            "deleted": bool(clip.get("deleted")) if clip.get("deleted") is not None else None,
            "pinned": bool(clip.get("pinned")) if clip.get("pinned") is not None else None,
            "in_collection": in_collection,
            "collection_sync_id": clip_collection_sync_id,
            "sync_version": clip.get("sync_version"),
            "source_device": clip.get("source_device"),
        }

    collections = []
    for collection in state.get("collections", []):
        sync_id = collection.get("sync_id") or ""
        collections.append(
            {
                "id": collection.get("id"),
                "name": collection.get("name") or "",
                "color": (collection.get("color") or "").upper(),
                "sync_id": sync_id,
                "deleted": bool(collection.get("deleted")),
                "sync_version": int(collection.get("sync_version") or 0),
                "clip_count": int(collection.get("clip_count") or 0),
            }
        )

    return {
        "collection_sync_id": collection_sync_id,
        "collection_deleted": collection_deleted,
        "clips": clips,
        "collections": collections,
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
        try:
            resp = send_cmd(host, port, {"cmd": "state"}, timeout=max(2.0, min(8.0, timeout_s / 3.0)))
        except Exception as exc:
            errors = [f"state command exception: {exc}"]
            time.sleep(0.25)
            continue
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


def wait_for_clip_state(
    host: str,
    port: int,
    content: str,
    timeout_s: float,
    *,
    expected_exists: bool | None = None,
    expected_deleted: bool | None = None,
) -> tuple[bool, dict, list[str]]:
    start = time.time()
    last_map = {}
    errors = []

    while time.time() - start < timeout_s:
        try:
            resp = send_cmd(host, port, {"cmd": "state"}, timeout=max(2.0, min(8.0, timeout_s / 3.0)))
        except Exception as exc:
            errors = [f"state command exception: {exc}"]
            time.sleep(0.2)
            continue

        if not resp.get("ok"):
            errors = [f"state command failed: {resp.get('message')}"]
            time.sleep(0.2)
            continue

        last_map = state_to_map(resp.get("state") or {})
        clip = last_map.get("clips", {}).get(content, {})
        local_errors = []

        if expected_exists is not None and bool(clip.get("exists")) is not expected_exists:
            local_errors.append(
                f"{content} exists={clip.get('exists')} expected={expected_exists}"
            )

        if expected_deleted is not None and bool(clip.get("deleted")) is not expected_deleted:
            local_errors.append(
                f"{content} deleted={clip.get('deleted')} expected={expected_deleted}"
            )

        if not local_errors:
            return True, last_map, []

        errors = local_errors
        time.sleep(0.2)

    return False, last_map, errors


def run_phase(apply_host: str, apply_port: int, verify_host: str, verify_port: int, phase: int, timeout_s: float) -> tuple[bool, dict, list[str], str]:
    try:
        apply_resp = send_cmd(apply_host, apply_port, {"cmd": "phase", "phase": phase}, timeout=max(20.0, timeout_s))
    except Exception as exc:
        return False, {}, [f"apply phase {phase} exception: {exc}"], "apply"
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
    for collection in state_map.get("collections", []):
        print(
            f"[{label}] coll sync_id={collection.get('sync_id')} name={collection.get('name')}"
            f" color={collection.get('color')} deleted={collection.get('deleted')} sync_version={collection.get('sync_version')} clip_count={collection.get('clip_count')}"
        )


def wait_for_collection_state(
    host: str,
    port: int,
    sync_id: str,
    timeout_s: float,
    *,
    expected_name: str | None = None,
    expected_color: str | None = None,
    expected_deleted: bool | None = None,
) -> tuple[bool, dict, list[str]]:
    start = time.time()
    last_map = {}
    errors = []
    while time.time() - start < timeout_s:
        try:
            resp = send_cmd(host, port, {"cmd": "state"}, timeout=max(2.0, min(8.0, timeout_s / 3.0)))
        except Exception as exc:
            errors = [f"state command exception: {exc}"]
            time.sleep(0.2)
            continue

        if not resp.get("ok"):
            errors = [f"state command failed: {resp.get('message')}"]
            time.sleep(0.2)
            continue

        last_map = state_to_map(resp.get("state") or {})
        target = None
        for coll in last_map.get("collections", []):
            if coll.get("sync_id") == sync_id:
                target = coll
                break

        local_errors = []
        if target is None:
            local_errors.append(f"missing collection sync_id={sync_id}")
        else:
            if expected_name is not None and target.get("name") != expected_name:
                local_errors.append(
                    f"name={target.get('name')} expected={expected_name}"
                )
            if expected_color is not None and (target.get("color") or "").upper() != expected_color.upper():
                local_errors.append(
                    f"color={target.get('color')} expected={expected_color.upper()}"
                )
            if expected_deleted is not None and bool(target.get("deleted")) is not expected_deleted:
                local_errors.append(
                    f"deleted={target.get('deleted')} expected={expected_deleted}"
                )

        if not local_errors:
            return True, last_map, []

        errors = local_errors
        time.sleep(0.2)

    return False, last_map, errors


def expect_collection_count(state_map: dict, sync_id: str, expected_count: int) -> tuple[bool, list[str]]:
    target = None
    for coll in state_map.get("collections", []):
        if coll.get("sync_id") == sync_id:
            target = coll
            break
    if target is None:
        return False, [f"missing collection sync_id={sync_id}"]
    actual_count = int(target.get("clip_count") or 0)
    if actual_count != expected_count:
        return False, [f"clip_count={actual_count} expected={expected_count}"]
    return True, []


def ensure_seed_and_toggle(host: str, port: int):
    # force metadata toggle on in case config drifted
    resp = send_cmd(host, port, {"cmd": "set_metadata_sync", "enabled": True}, timeout=20.0)
    if not resp.get("ok"):
        raise RuntimeError(f"set_metadata_sync failed on {host}:{port}: {resp.get('message')}")
    resp = send_cmd(host, port, {"cmd": "seed"}, timeout=30.0)
    if not resp.get("ok"):
        raise RuntimeError(f"seed failed on {host}:{port}: {resp.get('message')}")


def run_delete_recopy_cycle(
    apply_host: str,
    apply_port: int,
    verify_host: str,
    verify_port: int,
    timeout_s: float,
    cycle_index: int,
) -> tuple[bool, list[str]]:
    errors = []

    copy_resp = send_cmd(
        apply_host,
        apply_port,
        {"cmd": "copy_text", "content": QA_DELETE_RECOPY_TEXT},
        timeout=max(8.0, timeout_s),
    )
    if not copy_resp.get("ok"):
        return False, [f"cycle {cycle_index}: copy_text failed: {copy_resp.get('message')}"]

    ok_local_copy, _, local_copy_errors = wait_for_clip_state(
        apply_host,
        apply_port,
        QA_DELETE_RECOPY_TEXT,
        timeout_s,
        expected_exists=True,
        expected_deleted=False,
    )
    if not ok_local_copy:
        return False, [f"cycle {cycle_index}: apply device did not capture clipboard"] + local_copy_errors

    ok_remote_copy, _, remote_copy_errors = wait_for_clip_state(
        verify_host,
        verify_port,
        QA_DELETE_RECOPY_TEXT,
        timeout_s,
        expected_exists=True,
        expected_deleted=False,
    )
    if not ok_remote_copy:
        return False, [f"cycle {cycle_index}: verify device did not receive copied clip"] + remote_copy_errors

    delete_resp = send_cmd(
        apply_host,
        apply_port,
        {"cmd": "delete_clip", "content": QA_DELETE_RECOPY_TEXT},
        timeout=max(8.0, timeout_s),
    )
    if not delete_resp.get("ok"):
        return False, [f"cycle {cycle_index}: delete_clip failed: {delete_resp.get('message')}"]

    ok_local_delete, _, local_delete_errors = wait_for_clip_state(
        apply_host,
        apply_port,
        QA_DELETE_RECOPY_TEXT,
        timeout_s,
        expected_exists=False,
        expected_deleted=True,
    )
    if not ok_local_delete:
        return False, [f"cycle {cycle_index}: apply device did not mark clip deleted"] + local_delete_errors

    ok_remote_delete, _, remote_delete_errors = wait_for_clip_state(
        verify_host,
        verify_port,
        QA_DELETE_RECOPY_TEXT,
        timeout_s,
        expected_exists=False,
        expected_deleted=True,
    )
    if not ok_remote_delete:
        return False, [f"cycle {cycle_index}: verify device did not receive delete tombstone"] + remote_delete_errors

    return True, errors


def main():
    parser = argparse.ArgumentParser(description="Rigorous cross-device metadata sync QA harness")
    parser.add_argument("--mac-port", type=int, default=51901)
    parser.add_argument("--win-port", type=int, default=51902)
    parser.add_argument("--timeout", type=float, default=25.0)
    parser.add_argument("--rounds", type=int, default=8)
    parser.add_argument("--delete-recopy-rounds", type=int, default=12)
    args = parser.parse_args()

    mac = ("127.0.0.1", args.mac_port)
    win = ("127.0.0.1", args.win_port)

    # Ensure sync session is active before mutation phases.
    wait_for_connected(*mac, timeout_s=max(args.timeout * 2, 60.0))
    wait_for_connected(*win, timeout_s=max(args.timeout * 2, 60.0))

    time.sleep(1.0)
    ensure_seed_and_toggle(*mac)
    ensure_seed_and_toggle(*win)

    # Warm-up state prints
    mac_state = send_cmd(*mac, {"cmd": "state"}, timeout=10.0)
    win_state = send_cmd(*win, {"cmd": "state"}, timeout=10.0)
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
    delete_resp = send_cmd(mac[0], mac[1], {"cmd": "delete_collection"}, timeout=max(20.0, args.timeout))
    if not delete_resp.get("ok"):
        failures.append(("delete", "mac->win", None, [f"delete apply failed: {delete_resp.get('message')}"], {}))
    else:
        start = time.time()
        delete_ok = False
        last_map = {}
        delete_errors = []
        while time.time() - start < args.timeout:
            try:
                resp = send_cmd(win[0], win[1], {"cmd": "state"}, timeout=max(2.0, min(8.0, args.timeout / 3.0)))
            except Exception as exc:
                delete_errors = [f"state exception: {exc}"]
                time.sleep(0.25)
                continue
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

    # collection create/rename and count propagation checks
    extra_sync_id = "qa-sync-extra-propagation"
    create_resp = send_cmd(
        mac[0],
        mac[1],
        {
            "cmd": "make_collection",
            "suffix": "propagation",
            "name": "QA Propagation One",
            "color": "#34C759",
        },
        timeout=max(20.0, args.timeout),
    )
    if not create_resp.get("ok"):
        failures.append(("create", "mac->win", None, [f"create failed: {create_resp.get('message')}"], {}))
    else:
        ok, state_map, errors = wait_for_collection_state(
            win[0], win[1], extra_sync_id, args.timeout, expected_name="QA Propagation One", expected_color="#34C759", expected_deleted=False
        )
        if ok:
            print("COLLECTION CREATE mac->win: PASS")
        else:
            print("COLLECTION CREATE mac->win: FAIL")
            print_state("WIN:create-fail", state_map)
            for e in errors:
                print(f"  - {e}")
            failures.append(("create", "mac->win", None, errors, state_map))

    rename_resp = send_cmd(
        win[0],
        win[1],
        {
            "cmd": "make_collection",
            "suffix": "propagation",
            "name": "QA Propagation Renamed",
            "color": "#FF9500",
        },
        timeout=max(20.0, args.timeout),
    )
    if not rename_resp.get("ok"):
        failures.append(("rename", "win->mac", None, [f"rename failed: {rename_resp.get('message')}"], {}))
    else:
        ok, state_map, errors = wait_for_collection_state(
            mac[0], mac[1], extra_sync_id, args.timeout, expected_name="QA Propagation Renamed", expected_color="#FF9500", expected_deleted=False
        )
        if ok:
            print("COLLECTION RENAME/COLOR win->mac: PASS")
        else:
            print("COLLECTION RENAME/COLOR win->mac: FAIL")
            print_state("MAC:rename-fail", state_map)
            for e in errors:
                print(f"  - {e}")
            failures.append(("rename", "win->mac", None, errors, state_map))

    # ensure clip_count propagation for collection sidebar badge
    phase_resp = send_cmd(mac[0], mac[1], {"cmd": "phase", "phase": 2}, timeout=max(20.0, args.timeout))
    if not phase_resp.get("ok"):
        failures.append(("count-phase", "mac->win", None, [f"phase apply failed: {phase_resp.get('message')}"], {}))
    else:
        ok, state_map, errors = wait_for_collection_state(
            win[0], win[1], QA_COLLECTION_SYNC_ID, args.timeout, expected_deleted=False
        )
        if ok:
            count_ok, count_errors = expect_collection_count(state_map, QA_COLLECTION_SYNC_ID, 1)
            if count_ok:
                print("COLLECTION COUNT mac->win phase=2: PASS")
            else:
                print("COLLECTION COUNT mac->win phase=2: FAIL")
                print_state("WIN:count-fail", state_map)
                for e in count_errors:
                    print(f"  - {e}")
                failures.append(("count-phase", "mac->win", 2, count_errors, state_map))
        else:
            print("COLLECTION COUNT mac->win phase=2: FAIL")
            print_state("WIN:count-fail", state_map)
            for e in errors:
                print(f"  - {e}")
            failures.append(("count-phase", "mac->win", 2, errors, state_map))

    # clip delete propagation and identical recopy reliability checks
    for i in range(1, args.delete_recopy_rounds + 1):
        ok, errors = run_delete_recopy_cycle(
            mac[0],
            mac[1],
            win[0],
            win[1],
            args.timeout,
            i,
        )
        if ok:
            print(f"DELETE/RECOPY ROUND {i} A mac->win: PASS")
        else:
            print(f"DELETE/RECOPY ROUND {i} A mac->win: FAIL")
            for e in errors:
                print(f"  - {e}")
            failures.append(("delete-recopy", "mac->win", i, errors, {}))

        ok, errors = run_delete_recopy_cycle(
            win[0],
            win[1],
            mac[0],
            mac[1],
            args.timeout,
            i,
        )
        if ok:
            print(f"DELETE/RECOPY ROUND {i} B win->mac: PASS")
        else:
            print(f"DELETE/RECOPY ROUND {i} B win->mac: FAIL")
            for e in errors:
                print(f"  - {e}")
            failures.append(("delete-recopy", "win->mac", i, errors, {}))

    if failures:
        print(f"QA RESULT: FAIL ({len(failures)} failure(s))")
        sys.exit(1)

    print("QA RESULT: PASS")


if __name__ == "__main__":
    main()
