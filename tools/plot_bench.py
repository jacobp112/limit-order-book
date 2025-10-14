"""Aggregate benchmark output into a summary, a Markdown table and charts.

Usage:
    python tools/plot_bench.py RUNS_DIR CRITERION_DIR OUT_DIR

RUNS_DIR holds one or more latency-harness JSON files (one per independent
run). CRITERION_DIR is Criterion's output (target/criterion); the per-workload
estimates are copied into OUT_DIR/criterion/ so the summary can be rebuilt
from committed files. Writes OUT_DIR/summary.json, latency.svg and
throughput.svg, and prints a Markdown table.

Latency percentiles are the median across runs of each run's percentile;
the p50 range across runs is reported alongside. Throughput is Criterion's
mean time per batch divided by batch size, with its 95% confidence interval.
Values are rounded to at most three significant figures: run-to-run
variation on a laptop is far larger than that.
"""

import json
import math
import shutil
import statistics
import sys
from pathlib import Path

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402

# Reference palette (light surface); p50/p95/p99 use an ordinal one-hue ramp
# validated with the dataviz palette checker (--ordinal, light).
SURFACE = "#fcfcfb"
INK = "#0b0b0b"
INK_2 = "#52514e"
GRID = "#e4e3df"
RANGE = "#c9c8c2"
P_COLORS = {"p50": "#86b6ef", "p95": "#2a78d6", "p99": "#104281"}
BAR = "#2a78d6"


def sig(x: float, digits: int = 3) -> float:
    if x == 0:
        return 0.0
    return round(x, digits - 1 - int(math.floor(math.log10(abs(x)))))


def fmt_ns(ns: float) -> str:
    if ns >= 1000:
        return f"{sig(ns / 1000):g} µs"
    return f"{sig(ns):g} ns"


# Workloads whose single command does much more work than the others.
LABELS = {"sweep": "sweep (50 fills)"}


def label(name: str) -> str:
    return LABELS.get(name, name)


def style(ax):
    ax.set_facecolor(SURFACE)
    for side in ("top", "right", "left"):
        ax.spines[side].set_visible(False)
    ax.spines["bottom"].set_color(RANGE)
    ax.tick_params(colors=INK_2, labelsize=9, length=0)
    ax.grid(axis="x", color=GRID, linewidth=0.8)
    ax.set_axisbelow(True)


def footer(fig, data):
    fig.text(
        0.01,
        0.01,
        f"{data['machine']}\n"
        f"{data['os']}/{data['arch']} · {data['profile']} build · commit {data['commit'][:7]} · "
        f"timer resolution {data['timer_resolution_ns']} ns\n"
        "Latency sample = mean time per command over a chunk of consecutive commands.",
        fontsize=7.5,
        color=INK_2,
        va="bottom",
        linespacing=1.4,
    )


def save(fig, out: Path):
    """SVG without a creation date and with LF line endings, so output is reproducible."""
    fig.savefig(out, facecolor=SURFACE, metadata={"Date": None})
    plt.close(fig)
    out.write_bytes(out.read_bytes().replace(b"\r\n", b"\n"))


def aggregate(runs):
    """Median across runs of each percentile, keyed like a single run."""
    first = runs[0]
    rows = []
    for i, w in enumerate(first["workloads"]):
        per_run = [r["workloads"][i] for r in runs]
        assert all(p["name"] == w["name"] for p in per_run), "runs list different workloads"
        row = {k: w[k] for k in ("name", "description", "commands_per_round", "chunk")}
        row["runs"] = len(runs)
        row["samples_per_run"] = w["samples"]
        for key in ("p50_ns", "p95_ns", "p99_ns", "max_ns"):
            row[key] = statistics.median(p[key] for p in per_run)
        row["p50_min_ns"] = min(p["p50_ns"] for p in per_run)
        row["p50_max_ns"] = max(p["p50_ns"] for p in per_run)
        rows.append(row)
    meta = {k: first[k] for k in first if k != "workloads"}
    return {**meta, "workloads": rows}


def criterion(data, crit_dir: Path, out_dir: Path):
    """Attach Criterion mean and 95% CI per command; copy the estimates."""
    (out_dir / "criterion").mkdir(parents=True, exist_ok=True)
    for row in data["workloads"]:
        src = crit_dir / "engine" / row["name"] / "new" / "estimates.json"
        dst = out_dir / "criterion" / f"{row['name']}.json"
        if src.exists():
            shutil.copyfile(src, dst)
        est = json.loads(dst.read_text(encoding="utf-8"))["mean"]
        n = row["commands_per_round"]
        row["criterion_mean_ns"] = est["point_estimate"] / n
        row["criterion_ci_ns"] = [
            est["confidence_interval"]["lower_bound"] / n,
            est["confidence_interval"]["upper_bound"] / n,
        ]


def latency_chart(data, out: Path):
    rows = data["workloads"]
    names = [label(r["name"]) for r in rows][::-1]
    rows = rows[::-1]
    fig, ax = plt.subplots(figsize=(8, 4.2), dpi=100)
    fig.patch.set_facecolor(SURFACE)
    style(ax)
    for y, r in enumerate(rows):
        ax.plot([r["p50_ns"], r["p99_ns"]], [y, y], color=RANGE, linewidth=2, zorder=1,
                solid_capstyle="round")
        for key in ("p50", "p95", "p99"):
            ax.scatter(r[f"{key}_ns"], y, s=64, color=P_COLORS[key], zorder=2,
                       edgecolors=SURFACE, linewidths=2, label=key if y == 0 else None)
        ax.text(r["p99_ns"] * 1.15, y, fmt_ns(r["p50_ns"]) + " median", va="center",
                fontsize=8.5, color=INK_2)
    ax.set_xscale("log")
    ax.set_yticks(range(len(names)), names, color=INK, fontsize=9.5)
    ax.set_xlabel("time per command (log scale)", color=INK_2, fontsize=9)
    lo = min(r["p50_ns"] for r in rows) / 2
    hi = max(r["p99_ns"] for r in rows) * 6
    ax.set_xlim(lo, hi)
    ax.xaxis.set_major_formatter(matplotlib.ticker.FuncFormatter(lambda v, _: fmt_ns(v)))
    ax.set_title(f"Engine latency by workload: p50, p95, p99 (median of {rows[0]['runs']} runs)",
                 loc="left", color=INK,
                 fontsize=12, pad=26)
    ax.legend(loc="lower left", bbox_to_anchor=(0, 1.0), ncol=3, frameon=False,
              fontsize=9, labelcolor=INK, handletextpad=0.3, columnspacing=1.2)
    fig.subplots_adjust(left=0.19, right=0.97, top=0.84, bottom=0.24)
    footer(fig, data)
    save(fig, out)


def throughput_chart(data, out: Path):
    rows = data["workloads"][::-1]
    fig, ax = plt.subplots(figsize=(8, 3.8), dpi=100)
    fig.patch.set_facecolor(SURFACE)
    style(ax)
    values = [1e3 / r["criterion_mean_ns"] for r in rows]  # millions per second
    lo = [v - 1e3 / r["criterion_ci_ns"][1] for v, r in zip(values, rows)]
    hi = [1e3 / r["criterion_ci_ns"][0] - v for v, r in zip(values, rows)]
    bars = ax.barh(range(len(rows)), values, height=0.6, color=BAR)
    ax.errorbar(values, range(len(rows)), xerr=[lo, hi], fmt="none", ecolor=INK_2,
                elinewidth=1, capsize=3)
    for b, v, h in zip(bars, values, hi):
        ax.text(v + h + max(values) * 0.015, b.get_y() + b.get_height() / 2,
                f"{sig(v, 2):g} M/s", va="center", fontsize=8.5, color=INK_2)
    ax.set_yticks(range(len(rows)), [label(r["name"]) for r in rows], color=INK, fontsize=9.5)
    ax.set_xlabel("commands per second (millions), single thread; whiskers = 95% CI",
                  color=INK_2, fontsize=9)
    ax.set_xlim(0, max(v + h for v, h in zip(values, hi)) * 1.18)
    ax.set_title("Engine throughput by workload (Criterion mean)", loc="left", color=INK,
                 fontsize=12, pad=10)
    fig.subplots_adjust(left=0.19, right=0.97, top=0.88, bottom=0.26)
    footer(fig, data)
    save(fig, out)


def table(data) -> str:
    lines = [
        "| Workload | p50 | p50 range across runs | p95 | p99 | Criterion mean | Throughput |",
        "|---|---:|---:|---:|---:|---:|---:|",
    ]
    for r in data["workloads"]:
        lines.append(
            f"| `{label(r['name'])}` | {fmt_ns(r['p50_ns'])} | "
            f"{fmt_ns(r['p50_min_ns'])} – {fmt_ns(r['p50_max_ns'])} | "
            f"{fmt_ns(r['p95_ns'])} | {fmt_ns(r['p99_ns'])} | {fmt_ns(r['criterion_mean_ns'])} | "
            f"{sig(1e3 / r['criterion_mean_ns'], 2):g} M/s |"
        )
    return "\n".join(lines)


def main():
    if len(sys.argv) != 4:
        sys.exit(__doc__)
    runs_dir, crit_dir, out = (Path(a) for a in sys.argv[1:])
    runs = [json.loads(p.read_text(encoding="utf-8")) for p in sorted(runs_dir.glob("*.json"))]
    if not runs:
        sys.exit(f"no run files in {runs_dir}")
    out.mkdir(parents=True, exist_ok=True)
    data = aggregate(runs)
    criterion(data, crit_dir, out)
    # Explicit LF so regenerating on Windows is byte-identical to the repository.
    (out / "summary.json").write_text(
        json.dumps(data, indent=2) + "\n", encoding="utf-8", newline="\n"
    )
    plt.rcParams["svg.fonttype"] = "none"
    plt.rcParams["font.family"] = ["DejaVu Sans"]
    plt.rcParams["svg.hashsalt"] = "lob"  # stable ids so re-renders diff cleanly
    latency_chart(data, out / "latency.svg")
    throughput_chart(data, out / "throughput.svg")
    print(table(data))


if __name__ == "__main__":
    main()
