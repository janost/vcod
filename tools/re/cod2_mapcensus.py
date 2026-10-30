"""Census of material/techset use on stock MP maps.
usage: python3 -I mapcensus.py <bsp dir> <xmodel dir> mats.json tech.json"""
import sys, os, struct, re, json, glob, collections as C
bspdir, xmdir, mj, tj = sys.argv[1:5]
M = {m['name']: m for m in json.load(open(mj))}; T = json.load(open(tj))
def lump(d, i): l, o = struct.unpack_from('<II', d, 8 + 8 * i); return d[o:o + l]
world = C.Counter(); worldsurf = C.Counter(); models = C.Counter(); flagmix = C.Counter()
xmcache = {}
def xm_mats(name):
    if name in xmcache: return xmcache[name]
    p = os.path.join(xmdir, name)
    if not os.path.exists(p): xmcache[name] = None; return None
    s = re.findall(rb'[\x20-\x7e]{3,}', open(p, 'rb').read())
    xmcache[name] = [x.decode() for x in s if x.decode() in M]; return xmcache[name]
nomodel = C.Counter()
for p in sorted(glob.glob(os.path.join(bspdir, '*.d3dbsp'))):
    d = open(p, 'rb').read(); assert d[:4] == b'IBSP' and struct.unpack_from('<I', d, 4)[0] == 4
    mats = lump(d, 0); assert len(mats) % 72 == 0
    names = []
    for i in range(len(mats) // 72):
        n = mats[72 * i:72 * i + 64].split(b'\0')[0].decode(); sf, ct = struct.unpack_from('<II', mats, 72 * i + 64); names.append(n)
        if n in M: flagmix[(sf == M[n]['surfaceFlags'], ct == M[n]['contents'])] += 1
        else: flagmix['missing'] += 1
    soups = lump(d, 7); assert len(soups) % 16 == 0
    for i in range(len(soups) // 16):
        mat, lm, fv, vc, ic, fi = struct.unpack_from('<hhIHHI', soups, 16 * i)
        world[names[mat]] += ic // 3; worldsurf[(names[mat], lm != 0x1f and lm >= 0)] += ic // 3
    ents = lump(d, 37).decode('latin1')
    for e in re.findall(r'\{([^{}]*)\}', ents):
        kv = dict(re.findall(r'"([^"]*)"\s+"([^"]*)"', e))
        if kv.get('classname') == 'misc_model':
            mm = xm_mats(kv['model'].replace('xmodel/', ''))
            if mm is None: nomodel[kv['model']] += 1; continue
            for n in mm: models[n] += 1
print('BSP material lump flags equal to material file (surfaceFlags, contents):', flagmix)
print('misc_model without xmodel file', sum(nomodel.values()))
tot = sum(world.values()); print('world triangles', tot)
def techset_of(n): return M[n]['techset'] if n in M else '?'
def rank(counter, title, total):
    print(title)
    ts = C.Counter()
    for n, c in counter.items(): ts[techset_of(n)] += c
    acc = 0
    for t, c in ts.most_common(): acc += c; print('   %-32s %8d %5.1f%% cum %5.1f%%' % (t, c, 100 * c / total, 100 * acc / total))
    return ts
wts = rank(world, 'world triangles by techset', tot)
mts = rank(models, 'static model material references by techset', sum(models.values()))
print('lightmapped share of world tris', sum(c for (n, lm), c in worldsurf.items() if lm) / tot)
# techniques actually run: world surfaces with a lightmap use the lightmap types, models use the smodel types
def tech(ts, ty): return T['techsets'].get(ts, {}).get(ty)
tw = C.Counter()
for (n, lm), c in worldsurf.items():
    ts = techset_of(n)
    for ty in (['lightmap exp fog'] if lm else ['lightprobe exp fog']) + ['depth prepass', 'sky', 'emissive exp fog', 'emissive feathered exp fog', 'unlit exp fog']:
        t = tech(ts, ty)
        if t: tw[(ty, t)] += c
print('world triangles by (type, technique), exp fog shown')
for k, c in tw.most_common(40): print('   ', k, c)
tm = C.Counter()
for n, c in models.items():
    ts = techset_of(n)
    for ty in ['lightprobe smodel cached exp fog', 'emissive exp fog', 'emissive feathered exp fog']:
        t = tech(ts, ty)
        if t: tm[(ty, t)] += c
print('static model refs by (type, technique)')
for k, c in tm.most_common(30): print('   ', k, c)
