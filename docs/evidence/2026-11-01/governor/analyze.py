"""Offline analysis of pebs_overhead BLOCK lines (fixed and cap=N modes).

usage: python3 -I analyze.py LOG...

Fixed-period logs: per-period paired CPU delta, stolen-time (jitter) and the
per-sample cost regression, as in docs/evidence/2026-11-01/overhead.
Governed logs (CONFIG ... cap=Some(N)): additionally the steady-state sample
rate after the first governor update, the one-time attach transient, the
collector's attach and steady CPU, and a steady-state projection.
"""
import random
import re
import statistics as st
import sys

random.seed(1)


def num(v):
    try:
        return int(v)
    except ValueError:
        return float(v)


def parse(path):
    cfg, tpn, rows = None, None, []
    for line in open(path):
        if line.startswith('CONFIG'):
            cfg = line.strip()
            m = re.search(r'tsc_per_ns=([\d.]+)', line)
            tpn = float(m[1]) if m else None
        if not line.startswith('BLOCK'):
            continue
        d = {}
        for k, v in re.findall(r'(\w+)=(\[[^\]]*\]|\S+)', line):
            if v.startswith('['):
                d[k] = [num(x) for x in v.strip('[]').split(',') if x.strip()]
            else:
                d[k] = v if k == 'kind' else num(v)
        rows.append(d)
    return cfg, tpn, rows


def boot_ci(xs, f=st.median, n=4000):
    bs = sorted(f(random.choices(xs, k=len(xs))) for _ in range(n))
    return bs[int(0.025 * n)], bs[int(0.975 * n)]


def linfit(x, y):
    mx, my = st.mean(x), st.mean(y)
    sxx = sum((a - mx) ** 2 for a in x)
    b = sum((a - mx) * (c - my) for a, c in zip(x, y)) / sxx
    return b, my - b * mx


def boot_slope(x, y, n=3000):
    idx = list(range(len(x)))
    out = []
    for _ in range(n):
        ii = random.choices(idx, k=len(idx))
        xs = [x[i] for i in ii]
        if len(set(xs)) < 2:
            continue
        out.append(linfit(xs, [y[i] for i in ii])[0])
    out.sort()
    return out[int(.025 * len(out))], out[int(.975 * len(out))]


def pct(x):
    return f'{100 * x:+.2f}%'


for path in sys.argv[1:]:
    cfg, tpn, rows = parse(path)
    print('=' * 100)
    print(path)
    print(cfg)
    rows = [r for r in rows if r['pair'] < 1 << 63]
    jitter = 'jitter=true' in cfg
    governed = 'cap=Some(' in cfg
    periods = sorted({r['period'] for r in rows}, reverse=True)
    fit_x, fit_y, fit_g = [], [], []
    for p in periods:
        pr = {}
        for r in rows:
            if r['period'] == p:
                pr.setdefault(r['pair'], {})[r['kind']] = r
        cpu_rel, coll_frac, rates, wall_rel, gap_frac = [], [], [], [], []
        settled_rate, transient, attach_ms, steady_coll, scales = [], [], [], [], []
        for pair, k in sorted(pr.items()):
            b, s = k['base'], k['sampled']
            n = s['accepted'] + s['rejected'] + s['dropped']
            rates.append(n / (s['cpu_ns'] / 1e9))
            cpu_rel.append(s['cpu_ns'] / b['cpu_ns'] - 1)
            wall_rel.append(s['wall_ns'] / b['wall_ns'] - 1)
            coll_frac.append(s['coll_cpu_ns'] / s['wall_ns'])
            fit_x.append(n / (s['cpu_ns'] / 1e9))
            fit_y.append(s['cpu_ns'] / b['cpu_ns'] - 1)
            if governed:
                settled_rate.append(s['settled_accepted'] / (s['settled_ns'] / 1e9))
                transient.append(s['accepted'] - s['settled_accepted'])
                attach_ms.append(s['attach_cpu_ns'] / 1e6)
                steady_coll.append((s['coll_cpu_ns'] - s['attach_cpu_ns']) / s['wall_ns'])
                scales.append(s['scale'])
            if jitter:
                # exclude >=64us bucket: preemption by other tasks (also in base)
                gs = sum(s['gap_tsc'][:7]) / tpn
                gb = sum(b['gap_tsc'][:7]) / tpn
                gap_frac.append((gs / s['cpu_ns']) - (gb / b['cpu_ns']))
                fit_g.append(((gs / s['cpu_ns']) - (gb / b['cpu_ns']), n / (s['cpu_ns'] / 1e9)))
        lo, hi = boot_ci(cpu_rel)
        print(f'period={p:>8} pairs={len(cpu_rel)} block_wall_s med={st.median(r["sampled"]["wall_ns"] for r in pr.values()) / 1e9:.2f} '
              f'samples/s med={st.median(rates):8.0f} cpu_delta med={pct(st.median(cpu_rel))} CI95[{pct(lo)},{pct(hi)}] '
              f'wall_delta med={pct(st.median(wall_rel))} collector={100 * st.median(coll_frac):.4f}%core (max {100 * max(coll_frac):.4f})')
        if jitter:
            flo, fhi = boot_ci(gap_frac)
            print(f'      stolen-time fraction delta med={100 * st.median(gap_frac):+.3f}% CI95[{100 * flo:+.3f},{100 * fhi:+.3f}]')
        if governed:
            rlo, rhi = boot_ci(settled_rate)
            clo, chi = boot_ci(steady_coll)
            print(f'      governed: final scale med={st.median(scales):.2f} settled samples/s med={st.median(settled_rate):.0f} '
                  f'CI95[{rlo:.0f},{rhi:.0f}] max={max(settled_rate):.0f}')
            print(f'      one-time per attach: transient samples med={st.median(transient):.0f}, '
                  f'collector attach CPU med={st.median(attach_ms):.1f} ms (2 samplers: workload + collector thread)')
            print(f'      collector steady CPU (excl. attach) med={100 * st.median(steady_coll):.4f}%core '
                  f'CI95[{100 * clo:.4f},{100 * chi:.4f}] max={100 * max(steady_coll):.4f}')
    if len(set(fit_x)) > 2 and len(periods) > 1:
        b, a = linfit(fit_x, fit_y)
        slo, shi = boot_slope(fit_x, fit_y)
        print(f'CPU-time regression: cost/sample = {b * 1e9:.0f} ns CI95[{slo * 1e9:.0f},{shi * 1e9:.0f}] intercept={100 * a:+.2f}%')
        if jitter:
            gx = [x for _, x in fit_g]
            gy = [y for y, _ in fit_g]
            gb_, ga_ = linfit(gx, gy)
            glo, ghi = boot_slope(gx, gy)
            print(f'Gap-time regression: cost/sample = {gb_ * 1e9:.0f} ns CI95[{glo * 1e9:.0f},{ghi * 1e9:.0f}] intercept={100 * ga_:+.3f}%')
