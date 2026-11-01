"""Offline analysis of pebs_overhead BLOCK lines. usage: analyze.py LOG..."""
import random
import re
import statistics as st
import sys

random.seed(1)


def parse(path):
    cfg, rows = None, []
    for line in open(path):
        if line.startswith('CONFIG'):
            cfg = line.strip()
            m = re.search(r'tsc_per_ns=([\d.]+)', line)
            tpn = float(m[1]) if m else None
        if not line.startswith('BLOCK'):
            continue
        d = {}
        for k, v in re.findall(r'(\w+)=(\[[^\]]*\]|\S+)', line):
            d[k] = [int(x) for x in v.strip('[]').split(',') if x.strip()] if v.startswith('[') else (v if k == 'kind' else int(v))
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


for path in sys.argv[1:]:
    cfg, tpn, rows = parse(path)
    print('=' * 100)
    print(path)
    print(cfg)
    rows = [r for r in rows if r['pair'] < 1 << 63]
    jitter = 'jitter=true' in cfg
    periods = sorted({r['period'] for r in rows}, reverse=True)
    fit_x, fit_y, fit_g = [], [], []
    for p in periods:
        pr = {}
        for r in rows:
            if r['period'] == p:
                pr.setdefault(r['pair'], {})[r['kind']] = r
        cpu_rel, gap_ps, coll_frac, rates, samples, wall_rel, gap_frac = [], [], [], [], [], [], []
        for pair, k in sorted(pr.items()):
            b, s = k['base'], k['sampled']
            n = s['accepted'] + s['rejected'] + s['dropped']
            samples.append(n)
            rates.append(n / (s['cpu_ns'] / 1e9))
            cpu_rel.append(s['cpu_ns'] / b['cpu_ns'] - 1)
            wall_rel.append(s['wall_ns'] / b['wall_ns'] - 1)
            coll_frac.append(s['coll_cpu_ns'] / s['wall_ns'])
            fit_x.append(n / (s['cpu_ns'] / 1e9))
            fit_y.append(s['cpu_ns'] / b['cpu_ns'] - 1)
            if jitter:
                # exclude >=64us bucket: preemption by other tasks (also in base)
                gs = sum(s['gap_tsc'][:7]) / tpn
                gb = sum(b['gap_tsc'][:7]) / tpn
                gap_ps.append((gs - gb) / n if n else 0)
                gap_frac.append((gs / s['cpu_ns']) - (gb / b['cpu_ns']))
                fit_g.append(((gs / s['cpu_ns']) - (gb / b['cpu_ns']), n / (s['cpu_ns'] / 1e9)))
        lo, hi = boot_ci(cpu_rel)
        print(f'period={p:>8} pairs={len(cpu_rel)} samples/s med={st.median(rates):8.0f} '
              f'cpu_delta med={100*st.median(cpu_rel):+6.2f}% CI95[{100*lo:+.2f},{100*hi:+.2f}] '
              f'wall_delta med={100*st.median(wall_rel):+6.2f}% collector={100*st.median(coll_frac):.4f}%core (max {100*max(coll_frac):.4f})')
        if jitter:
            glo, ghi = boot_ci(gap_ps)
            flo, fhi = boot_ci(gap_frac)
            print(f'      direct stolen time per sample (gap delta) med={st.median(gap_ps):7.0f} ns CI95[{glo:.0f},{ghi:.0f}]  '
                  f'gap-time fraction delta med={100*st.median(gap_frac):+.3f}% CI95[{100*flo:+.3f},{100*fhi:+.3f}]')
            # gap histogram means
            bk = [st.mean(pr[q]['base']['gaps'][i] for q in pr) for i in range(8)]
            sk = [st.mean(pr[q]['sampled']['gaps'][i] for q in pr) for i in range(8)]
            print('      mean gaps/block base   ', [round(x, 1) for x in bk])
            print('      mean gaps/block sampled', [round(x, 1) for x in sk], ' mean samples/block', round(st.mean(samples), 1))
    b, a = linfit(fit_x, fit_y)
    # bootstrap slope
    idx = list(range(len(fit_x)))
    slopes = []
    for _ in range(3000):
        ii = random.choices(idx, k=len(idx))
        xs = [fit_x[i] for i in ii]
        if len(set(xs)) < 2:
            continue
        slopes.append(linfit(xs, [fit_y[i] for i in ii])[0])
    slopes.sort()
    print(f'CPU-time regression: cost/sample = {b*1e9:.0f} ns CI95[{slopes[int(.025*len(slopes))]*1e9:.0f},{slopes[int(.975*len(slopes))]*1e9:.0f}] intercept={100*a:+.2f}%')
    if jitter:
        gx = [x for _, x in fit_g]
        gy = [y for y, _ in fit_g]
        gb_, ga_ = linfit(gx, gy)
        sl = []
        for _ in range(3000):
            ii = random.choices(range(len(gx)), k=len(gx))
            xs = [gx[i] for i in ii]
            if len(set(xs)) < 2:
                continue
            sl.append(linfit(xs, [gy[i] for i in ii])[0])
        sl.sort()
        print(f'Gap-time regression: cost/sample = {gb_*1e9:.0f} ns CI95[{sl[int(.025*len(sl))]*1e9:.0f},{sl[int(.975*len(sl))]*1e9:.0f}] intercept={100*ga_:+.3f}%')
