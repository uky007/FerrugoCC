#!/usr/bin/env python3
"""
angr_eval.py — Evaluate obfuscation resistance against symbolic execution (angr).

For each benchmark binary (L0 and L3), measures:
1. CFG recovery: nodes, edges, functions detected
2. Symbolic execution: can angr find a path to the expected exit code?
3. Time: how long does CFG recovery and symbolic execution take?

Usage: python3 scripts/eval/angr_eval.py <results_dir> <expected_file>
Output: <results_dir>/angr_eval.csv
"""

import angr
import claripy
import csv
import os
import sys
import time

def evaluate_binary(bin_path, expected_exit, timeout_sec=60):
    """Evaluate a single binary with angr."""
    result = {
        "cfg_nodes": 0,
        "cfg_edges": 0,
        "cfg_functions": 0,
        "cfg_time_ms": 0,
        "se_found": False,
        "se_paths_explored": 0,
        "se_time_ms": 0,
        "se_status": "not_run",
    }

    try:
        proj = angr.Project(bin_path, auto_load_libs=False)
    except Exception as e:
        result["se_status"] = f"load_error: {e}"
        return result

    # --- CFG Recovery ---
    t0 = time.monotonic()
    try:
        cfg = proj.analyses.CFGFast()
        result["cfg_nodes"] = cfg.graph.number_of_nodes()
        result["cfg_edges"] = cfg.graph.number_of_edges()
        result["cfg_functions"] = len(cfg.kb.functions)
    except Exception as e:
        result["se_status"] = f"cfg_error: {e}"
    result["cfg_time_ms"] = int((time.monotonic() - t0) * 1000)

    # --- Symbolic Execution ---
    # Try to find a path that reaches the expected exit code
    t1 = time.monotonic()
    try:
        state = proj.factory.entry_state()
        simgr = proj.factory.simgr(state)

        # Step with timeout
        deadline = time.monotonic() + timeout_sec
        paths_explored = 0

        while simgr.active and time.monotonic() < deadline:
            simgr.step()
            paths_explored += len(simgr.active)

            # Check deadended states for expected exit code
            for s in simgr.deadended:
                try:
                    # Check if exit code matches
                    ret_val = s.solver.eval(s.regs.rax)
                    # Exit code is return value mod 256
                    if (ret_val % 256) == (expected_exit % 256):
                        result["se_found"] = True
                        result["se_status"] = "found"
                        break
                except Exception:
                    pass

            if result["se_found"]:
                break

            # Limit total paths to prevent memory explosion
            if paths_explored > 10000:
                result["se_status"] = "path_explosion"
                break

        if not result["se_found"] and result["se_status"] == "not_run":
            if time.monotonic() >= deadline:
                result["se_status"] = "timeout"
            elif not simgr.active:
                result["se_status"] = "exhausted"
            else:
                result["se_status"] = "unknown"

        result["se_paths_explored"] = paths_explored

    except Exception as e:
        result["se_status"] = f"se_error: {str(e)[:100]}"

    result["se_time_ms"] = int((time.monotonic() - t1) * 1000)
    return result


def main():
    if len(sys.argv) < 3:
        print(f"Usage: {sys.argv[0]} <results_dir> <expected_file>", file=sys.stderr)
        sys.exit(1)

    results_dir = sys.argv[1]
    expected_file = sys.argv[2]
    timeout = int(sys.argv[3]) if len(sys.argv) > 3 else 60

    # Read expected exit codes
    expected = {}
    with open(expected_file) as f:
        for line in f:
            line = line.strip()
            if ":" in line:
                prog, code = line.split(":", 1)
                expected[prog] = int(code)

    out_csv = os.path.join(results_dir, "angr_eval.csv")
    with open(out_csv, "w", newline="") as f:
        writer = csv.writer(f)
        writer.writerow([
            "program", "condition",
            "cfg_nodes", "cfg_edges", "cfg_functions", "cfg_time_ms",
            "se_found", "se_paths_explored", "se_time_ms", "se_status",
        ])

        conditions = ["L0", "L3"]
        for cond in conditions:
            cond_dir = os.path.join(results_dir, "binaries", cond)
            if not os.path.isdir(cond_dir):
                print(f"  SKIP {cond}: directory not found")
                continue

            print(f"=== {cond} ===")
            for prog, exit_code in sorted(expected.items()):
                bin_path = os.path.join(cond_dir, prog)
                if not os.path.isfile(bin_path):
                    print(f"  SKIP {prog}: binary not found")
                    continue

                print(f"  {prog}...", end="", flush=True)
                result = evaluate_binary(bin_path, exit_code, timeout_sec=timeout)
                print(f" cfg={result['cfg_nodes']}n/{result['cfg_edges']}e "
                      f"se={result['se_status']} ({result['se_time_ms']}ms)")

                writer.writerow([
                    prog, cond,
                    result["cfg_nodes"], result["cfg_edges"],
                    result["cfg_functions"], result["cfg_time_ms"],
                    result["se_found"], result["se_paths_explored"],
                    result["se_time_ms"], result["se_status"],
                ])

    print(f"\nResults: {out_csv}")

    # Print summary
    print("\n=== Summary ===")
    with open(out_csv) as f:
        reader = csv.DictReader(f)
        rows = list(reader)

    for cond in ["L0", "L3"]:
        cond_rows = [r for r in rows if r["condition"] == cond]
        if not cond_rows:
            continue
        n = len(cond_rows)
        avg_nodes = sum(int(r["cfg_nodes"]) for r in cond_rows) / n
        avg_edges = sum(int(r["cfg_edges"]) for r in cond_rows) / n
        avg_funcs = sum(int(r["cfg_functions"]) for r in cond_rows) / n
        found = sum(1 for r in cond_rows if r["se_found"] == "True")
        timeout_count = sum(1 for r in cond_rows if r["se_status"] == "timeout")
        avg_se_time = sum(int(r["se_time_ms"]) for r in cond_rows) / n
        print(f"{cond}: cfg avg {avg_nodes:.0f} nodes, {avg_funcs:.0f} funcs | "
              f"SE found {found}/{n}, timeout {timeout_count}/{n}, "
              f"avg SE time {avg_se_time:.0f}ms")


if __name__ == "__main__":
    main()
