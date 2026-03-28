#!/usr/bin/env python3
"""
plot.py — Generate evaluation figures from CSV data.

Usage: python3 scripts/eval/plot.py <results_dir>

Generates:
  fig_size_overhead.png    — Binary size overhead vs L0 (L0-L4)
  fig_perf_overhead.png    — Execution time overhead vs L0 (L0-L4, with error bars)
  fig_reverse_metrics.png  — Reverse-engineering metrics normalized to L0
  fig_ablation.png         — Ablation: L3 vs L3-no-X (size + reverse metrics)

Only includes programs that passed correctness verification (from correctness.csv).
"""

import csv
import os
import sys
from collections import defaultdict

import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
import matplotlib.ticker as mticker


def read_csv(path):
    """Read a CSV file and return list of dicts."""
    with open(path, newline="") as f:
        return list(csv.DictReader(f))


def load_pass_set(results_dir):
    """Load set of (program, condition) pairs that passed correctness."""
    correctness_csv = os.path.join(results_dir, "correctness.csv")
    pass_set = set()
    if os.path.exists(correctness_csv):
        for r in read_csv(correctness_csv):
            if r.get("pass") == "true":
                pass_set.add((r["program"], r["condition"]))
    return pass_set


def is_passing(pass_set, prog, cond):
    """Check if (program, condition) passed correctness. Empty set = no filter."""
    if not pass_set:
        return True
    return (prog, cond) in pass_set


def mean(values):
    if not values:
        return 0.0
    return sum(values) / len(values)


def std(values):
    if len(values) < 2:
        return 0.0
    m = mean(values)
    return (sum((v - m) ** 2 for v in values) / (len(values) - 1)) ** 0.5


# ---- Level conditions (for main comparison) ----
LEVEL_CONDITIONS = ["L0", "L1", "L2", "L3", "L4"]
LEVEL_COLORS = ["#4CAF50", "#8BC34A", "#FFC107", "#FF9800", "#F44336"]

# ---- Ablation conditions ----
ABLATION_BASE = "L3"
ABLATION_CONDITIONS = [
    "L3", "L3-no-cff", "L3-no-str", "L3-no-arith",
    "L3-no-inl", "L3-no-outl",
]
ABLATION_COLORS = [
    "#2196F3", "#9C27B0", "#00BCD4", "#FF5722",
    "#795548", "#607D8B",
]


def plot_size_overhead(results_dir, rows, pass_set):
    """Fig 1: Binary size overhead relative to L0."""
    data = {}
    for r in rows:
        prog, cond = r["program"], r["condition"]
        if cond not in LEVEL_CONDITIONS:
            continue
        if not is_passing(pass_set, prog, cond):
            continue
        data.setdefault(prog, {})[cond] = int(r["size_bytes"])

    # Only include programs that have L0 data
    programs = sorted(p for p in data if "L0" in data[p])
    if not programs:
        print("  WARNING: no size data for level conditions")
        return

    fig, ax = plt.subplots(figsize=(max(10, len(programs) * 0.8), 6))
    bar_width = 0.15
    x_base = list(range(len(programs)))

    for i, cond in enumerate(LEVEL_CONDITIONS):
        ratios = []
        for prog in programs:
            l0 = data[prog]["L0"]
            val = data[prog].get(cond)
            if val is None:
                ratios.append(0)
            else:
                ratios.append(val / l0 if l0 > 0 else 1.0)
        x_pos = [x + i * bar_width for x in x_base]
        ax.bar(x_pos, ratios, bar_width, label=cond, color=LEVEL_COLORS[i])

    ax.set_xlabel("Benchmark Program")
    ax.set_ylabel("Size Ratio (relative to L0)")
    ax.set_title("Binary Size Overhead by Obfuscation Level")
    ax.set_xticks([x + bar_width * 2 for x in x_base])
    ax.set_xticklabels(programs, rotation=45, ha="right", fontsize=8)
    ax.axhline(y=1.0, color="gray", linestyle="--", alpha=0.5)
    ax.legend()
    ax.yaxis.set_major_formatter(mticker.FormatStrFormatter("%.1f"))
    fig.tight_layout()
    fig.savefig(os.path.join(results_dir, "fig_size_overhead.png"), dpi=150)
    plt.close(fig)
    print("  fig_size_overhead.png")


def plot_perf_overhead(results_dir, rows, pass_set):
    """Fig 2: Execution time overhead relative to L0 (with error bars)."""
    data = defaultdict(lambda: defaultdict(list))
    for r in rows:
        prog, cond = r["program"], r["condition"]
        if cond not in LEVEL_CONDITIONS:
            continue
        if not is_passing(pass_set, prog, cond):
            continue
        try:
            data[prog][cond].append(float(r.get("avg_ms", r.get("time_sec", 0))))
        except (ValueError, TypeError):
            pass

    programs = sorted(p for p in data if "L0" in data[p])
    if not programs:
        print("  WARNING: no performance data for level conditions")
        return

    fig, ax = plt.subplots(figsize=(max(10, len(programs) * 0.8), 6))
    bar_width = 0.15
    x_base = list(range(len(programs)))

    for i, cond in enumerate(LEVEL_CONDITIONS):
        means = []
        errs = []
        for prog in programs:
            l0_times = data[prog].get("L0", [0.001])
            l0_mean = mean(l0_times) if mean(l0_times) > 0 else 0.001
            times = data[prog].get(cond, [])
            if not times:
                means.append(0)
                errs.append(0)
            else:
                ratio = mean(times) / l0_mean
                err = std([t / l0_mean for t in times]) if len(times) > 1 else 0
                means.append(ratio)
                errs.append(err)
        x_pos = [x + i * bar_width for x in x_base]
        ax.bar(x_pos, means, bar_width, yerr=errs, label=cond,
               color=LEVEL_COLORS[i], capsize=2)

    ax.set_xlabel("Benchmark Program")
    ax.set_ylabel("Time Ratio (relative to L0)")
    ax.set_title("Execution Time Overhead by Obfuscation Level")
    ax.set_xticks([x + bar_width * 2 for x in x_base])
    ax.set_xticklabels(programs, rotation=45, ha="right", fontsize=8)
    ax.axhline(y=1.0, color="gray", linestyle="--", alpha=0.5)
    ax.legend()
    fig.tight_layout()
    fig.savefig(os.path.join(results_dir, "fig_perf_overhead.png"), dpi=150)
    plt.close(fig)
    print("  fig_perf_overhead.png")


def plot_reverse_metrics(results_dir, rows, pass_set):
    """Fig 3: Reverse-engineering metrics normalized to L0."""
    METRICS = ["nm_symbols", "strings_count", "label_count"]
    METRIC_LABELS = ["Symbols (nm)", "Strings", "Labels (BBs)"]

    data = defaultdict(lambda: defaultdict(dict))
    for r in rows:
        prog, cond = r["program"], r["condition"]
        if cond not in LEVEL_CONDITIONS:
            continue
        if not is_passing(pass_set, prog, cond):
            continue
        for m in METRICS:
            data[prog][cond][m] = int(r[m])

    programs = sorted(p for p in data if "L0" in data[p])
    if not programs:
        print("  WARNING: no reverse metrics data")
        return

    fig, axes = plt.subplots(1, len(METRICS), figsize=(5 * len(METRICS), 6),
                             sharey=False)

    for ax_idx, (metric, label) in enumerate(zip(METRICS, METRIC_LABELS)):
        ax = axes[ax_idx]
        bar_width = 0.15
        x_base = list(range(len(programs)))

        for i, cond in enumerate(LEVEL_CONDITIONS):
            ratios = []
            for prog in programs:
                l0_val = data[prog].get("L0", {}).get(metric, 1)
                val = data[prog].get(cond, {}).get(metric)
                if val is None:
                    ratios.append(0)
                else:
                    ratios.append(val / l0_val if l0_val > 0 else 1.0)
            x_pos = [x + i * bar_width for x in x_base]
            ax.bar(x_pos, ratios, bar_width, label=cond, color=LEVEL_COLORS[i])

        ax.set_title(label)
        ax.set_xlabel("Program")
        ax.set_xticks([x + bar_width * 2 for x in x_base])
        ax.set_xticklabels(programs, rotation=45, ha="right", fontsize=7)
        ax.axhline(y=1.0, color="gray", linestyle="--", alpha=0.5)
        ax.set_ylabel("Ratio (vs L0)")

    axes[0].legend(fontsize=8)
    fig.suptitle("Reverse-Engineering Metrics (Normalized to L0)", fontsize=13)
    fig.tight_layout()
    fig.savefig(os.path.join(results_dir, "fig_reverse_metrics.png"), dpi=150)
    plt.close(fig)
    print("  fig_reverse_metrics.png")


def plot_ablation(results_dir, size_rows, reverse_rows, pass_set):
    """Fig 4: Ablation — L3 vs L3-no-X for size + reverse metrics."""
    size_data = {}
    for r in size_rows:
        prog, cond = r["program"], r["condition"]
        if cond not in ABLATION_CONDITIONS:
            continue
        if not is_passing(pass_set, prog, cond):
            continue
        size_data.setdefault(prog, {})[cond] = int(r["size_bytes"])

    rev_data = defaultdict(lambda: defaultdict(dict))
    METRICS = ["nm_symbols", "strings_count", "label_count"]
    for r in reverse_rows:
        prog, cond = r["program"], r["condition"]
        if cond not in ABLATION_CONDITIONS:
            continue
        if not is_passing(pass_set, prog, cond):
            continue
        for m in METRICS:
            rev_data[prog][cond][m] = int(r[m])

    programs = sorted(
        p for p in (set(size_data.keys()) | set(rev_data.keys()))
        if "L3" in size_data.get(p, {}) or "L3" in rev_data.get(p, {})
    )
    if not programs:
        print("  WARNING: no ablation data")
        return

    fig, axes = plt.subplots(2, 2, figsize=(14, 10))

    # Panel 1: Size ratio vs L3
    ax = axes[0][0]
    bar_width = 0.12
    x_base = list(range(len(programs)))
    for i, cond in enumerate(ABLATION_CONDITIONS):
        ratios = []
        for prog in programs:
            l3_val = size_data.get(prog, {}).get("L3", 1)
            val = size_data.get(prog, {}).get(cond)
            if val is None:
                ratios.append(0)
            else:
                ratios.append(val / l3_val if l3_val > 0 else 1.0)
        x_pos = [x + i * bar_width for x in x_base]
        ax.bar(x_pos, ratios, bar_width, label=cond, color=ABLATION_COLORS[i])
    ax.set_title("Binary Size (relative to L3)")
    ax.set_xticks([x + bar_width * 2.5 for x in x_base])
    ax.set_xticklabels(programs, rotation=45, ha="right", fontsize=7)
    ax.axhline(y=1.0, color="gray", linestyle="--", alpha=0.5)
    ax.legend(fontsize=7)

    # Panels 2-4: Reverse metrics vs L3
    for panel_idx, (metric, label) in enumerate(zip(METRICS,
            ["Symbols (nm)", "Strings", "Labels"])):
        row, col = divmod(panel_idx + 1, 2)
        ax = axes[row][col]
        for i, cond in enumerate(ABLATION_CONDITIONS):
            ratios = []
            for prog in programs:
                l3_val = rev_data.get(prog, {}).get("L3", {}).get(metric, 1)
                val = rev_data.get(prog, {}).get(cond, {}).get(metric)
                if val is None:
                    ratios.append(0)
                else:
                    ratios.append(val / l3_val if l3_val > 0 else 1.0)
            x_pos = [x + i * bar_width for x in x_base]
            ax.bar(x_pos, ratios, bar_width, label=cond,
                   color=ABLATION_COLORS[i])
        ax.set_title(f"{label} (relative to L3)")
        ax.set_xticks([x + bar_width * 2.5 for x in x_base])
        ax.set_xticklabels(programs, rotation=45, ha="right", fontsize=7)
        ax.axhline(y=1.0, color="gray", linestyle="--", alpha=0.5)

    fig.suptitle("Ablation Study: Effect of Disabling Individual Passes",
                 fontsize=13)
    fig.tight_layout()
    fig.savefig(os.path.join(results_dir, "fig_ablation.png"), dpi=150)
    plt.close(fig)
    print("  fig_ablation.png")


def plot_decompile_summary(results_dir, rows, pass_set):
    """Fig 5: Decompile eval summary — grouped bar chart per level."""
    METRICS = ["func_symbols", "total_instructions", "invalid_instructions", "readable_symbols"]
    LABELS = ["Function Symbols", "Total Instructions", "Invalid Instructions", "Readable Symbols"]

    data = defaultdict(lambda: defaultdict(list))
    for r in rows:
        prog, cond = r["program"], r["condition"]
        if cond not in LEVEL_CONDITIONS:
            continue
        if not is_passing(pass_set, prog, cond):
            continue
        for m in METRICS:
            try:
                data[cond][m].append(int(r[m]))
            except (ValueError, KeyError):
                pass

    if not data:
        print("  WARNING: no decompile eval data")
        return

    fig, axes = plt.subplots(2, 2, figsize=(12, 8))
    for idx, (metric, label) in enumerate(zip(METRICS, LABELS)):
        row, col = divmod(idx, 2)
        ax = axes[row][col]
        conds = [c for c in LEVEL_CONDITIONS if c in data]
        avgs = [mean(data[c][metric]) for c in conds]
        colors = [LEVEL_COLORS[LEVEL_CONDITIONS.index(c)] for c in conds]
        bars = ax.bar(conds, avgs, color=colors, edgecolor="white")
        ax.set_title(label, fontsize=11)
        ax.set_ylabel("Average count")
        # Add value labels on bars
        for bar, val in zip(bars, avgs):
            if val > 0:
                ax.text(bar.get_x() + bar.get_width() / 2, bar.get_height(),
                        f"{val:.0f}", ha="center", va="bottom", fontsize=8)

    fig.suptitle("Static Analysis Proxy Metrics by Obfuscation Level", fontsize=13)
    fig.tight_layout()
    fig.savefig(os.path.join(results_dir, "fig_decompile_summary.png"), dpi=150)
    plt.close(fig)
    print("  fig_decompile_summary.png")


def plot_compile_time(results_dir):
    """Fig 6: Compilation time by level."""
    csv_path = os.path.join(results_dir, "compile_time.csv")
    if not os.path.exists(csv_path):
        print("  WARNING: compile_time.csv not found")
        return

    data = defaultdict(list)
    for r in read_csv(csv_path):
        try:
            ms = int(r["compile_ms"])
            if ms > 0:
                data[r["condition"]].append(ms)
        except (ValueError, KeyError, TypeError):
            pass

    if not data:
        print("  WARNING: no compile time data")
        return

    conds = [c for c in LEVEL_CONDITIONS if c in data]
    avgs = [mean(data[c]) for c in conds]
    colors = [LEVEL_COLORS[LEVEL_CONDITIONS.index(c)] for c in conds]

    fig, ax = plt.subplots(figsize=(8, 5))
    bars = ax.bar(conds, avgs, color=colors, edgecolor="white")
    ax.set_ylabel("Average Compilation Time (ms)")
    ax.set_xlabel("Obfuscation Level")
    ax.set_title("Compilation Time Overhead")

    for bar, val in zip(bars, avgs):
        label = f"{val:.0f}ms"
        if val > 100:
            label = f"{val/1000:.1f}s"
        ax.text(bar.get_x() + bar.get_width() / 2, bar.get_height(),
                label, ha="center", va="bottom", fontsize=9)

    # Use log scale if L4 is much larger
    if avgs and max(avgs) > 10 * min(a for a in avgs if a > 0):
        ax.set_yscale("log")

    fig.tight_layout()
    fig.savefig(os.path.join(results_dir, "fig_compile_time.png"), dpi=150)
    plt.close(fig)
    print("  fig_compile_time.png")


def plot_code_text_expansion(results_dir, rows, pass_set):
    """Fig 7: .text section expansion by level (paper main figure)."""
    data = defaultdict(lambda: defaultdict(list))
    for r in rows:
        prog, cond = r["program"], r["condition"]
        if cond not in LEVEL_CONDITIONS:
            continue
        if not is_passing(pass_set, prog, cond):
            continue
        try:
            data[cond]["text"].append(int(r.get("text_bytes", 0)))
            data[cond]["total"].append(int(r.get("size_bytes", 0)))
        except (ValueError, KeyError):
            pass

    if not data or "L0" not in data:
        print("  WARNING: no size data with text_bytes")
        return

    l0_text = mean(data["L0"]["text"])
    if l0_text == 0:
        print("  WARNING: L0 text_bytes is 0")
        return

    conds = [c for c in LEVEL_CONDITIONS if c in data]
    text_ratios = [mean(data[c]["text"]) / l0_text for c in conds]
    colors = [LEVEL_COLORS[LEVEL_CONDITIONS.index(c)] for c in conds]

    fig, ax = plt.subplots(figsize=(8, 5))
    bars = ax.bar(conds, text_ratios, color=colors, edgecolor="white")
    ax.set_ylabel("Code Section Expansion (×)")
    ax.set_xlabel("Obfuscation Level")
    ax.set_title("Code Section (.text) Expansion vs Normal")
    ax.axhline(y=1.0, color="gray", linestyle="--", alpha=0.5)

    for bar, val in zip(bars, text_ratios):
        ax.text(bar.get_x() + bar.get_width() / 2, bar.get_height(),
                f"{val:.1f}×", ha="center", va="bottom", fontsize=9)

    if max(text_ratios) > 20:
        ax.set_yscale("log")

    fig.tight_layout()
    fig.savefig(os.path.join(results_dir, "fig_code_expansion.png"), dpi=150)
    plt.close(fig)
    print("  fig_code_expansion.png")


def main():
    if len(sys.argv) < 2:
        print(f"Usage: {sys.argv[0]} <results_dir>", file=sys.stderr)
        sys.exit(1)

    results_dir = sys.argv[1]

    # Load correctness filter
    pass_set = load_pass_set(results_dir)
    if pass_set:
        print(f"  Loaded {len(pass_set)} passing (program, condition) pairs")
    else:
        print("  WARNING: no correctness data, including all entries")

    size_csv = os.path.join(results_dir, "size.csv")
    perf_csv = os.path.join(results_dir, "performance.csv")
    reverse_csv = os.path.join(results_dir, "reverse_metrics.csv")
    decompile_csv = os.path.join(results_dir, "decompile_eval.csv")

    print("=== Generating plots ===")

    if os.path.exists(size_csv):
        size_rows = read_csv(size_csv)
        plot_size_overhead(results_dir, size_rows, pass_set)
        plot_code_text_expansion(results_dir, size_rows, pass_set)
    else:
        size_rows = []
        print(f"  WARNING: {size_csv} not found")

    if os.path.exists(perf_csv):
        perf_rows = read_csv(perf_csv)
        plot_perf_overhead(results_dir, perf_rows, pass_set)
    else:
        perf_rows = []
        print(f"  WARNING: {perf_csv} not found")

    if os.path.exists(reverse_csv):
        reverse_rows = read_csv(reverse_csv)
        plot_reverse_metrics(results_dir, reverse_rows, pass_set)
    else:
        reverse_rows = []
        print(f"  WARNING: {reverse_csv} not found")

    if os.path.exists(decompile_csv):
        decompile_rows = read_csv(decompile_csv)
        plot_decompile_summary(results_dir, decompile_rows, pass_set)
    else:
        print(f"  WARNING: {decompile_csv} not found")

    if size_rows or reverse_rows:
        plot_ablation(results_dir, size_rows, reverse_rows, pass_set)

    plot_compile_time(results_dir)

    print("=== Done ===")


if __name__ == "__main__":
    main()
