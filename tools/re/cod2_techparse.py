"""Parse CoD2 techsets, techniques (dx9) and statemaps; print keyword census.
usage: python3 -I techparse.py <materials dir> -> writes tech.json"""
import sys, os, re, json, collections as C
root = sys.argv[1]
TOK = re.compile(r'//[^\n]*|/\*.*?\*/|"[^"]*"|[A-Za-z_$][\w$]*|-?\d+\.\d*|-?\d+|\S', re.S)
def toks(s): return [t for t in TOK.findall(s) if not t.startswith('//') and not t.startswith('/*')]
TYPES = ["depth prepass","sky","build floatz","unlit no fog","unlit linear fog","unlit exp fog","lightmap no fog","lightmap linear fog","lightmap exp fog","lightprobe no fog","lightprobe linear fog","lightprobe exp fog","lightprobe smodel no fog","lightprobe smodel linear fog","lightprobe smodel exp fog","lightprobe smodel cached no fog","lightprobe smodel cached linear fog","lightprobe smodel cached exp fog","pointlight no fog","pointlight linear fog","pointlight exp fog","emissive no fog","emissive linear fog","emissive exp fog","emissive feathered no fog","emissive feathered linear fog","emissive feathered exp fog","fakelight normal","fakelight view","solid wireframe","shaded wireframe","shadowcookie caster","shadowcookie receiver","debug bumpmap"]
def techset(path):
    t = toks(open(path).read()); i = 0; out = {}; labels = []
    while i < len(t):
        if t[i].startswith('"'):
            assert t[i][1:-1] in TYPES, t[i]; labels.append(t[i][1:-1]); assert t[i+1] == ':'; i += 2
        else:
            assert labels, (path, t[i]); assert t[i+1] == ';', (path, t[i:i+3])
            for l in labels: out[l] = t[i]
            labels = []; i += 2
    assert not labels; return out
def tech(path):
    t = toks(open(path).read()); i = 0; passes = []
    def ex(v):
        nonlocal i; assert t[i] == v, (path, i, t[i-3:i+3], v); i += 1
    while i < len(t):
        ex('{'); p = dict(stateMap=None, streams=[], vs=None, ps=None, vsargs=[], psargs=[])
        if t[i] == 'stateMap': i += 1; p['stateMap'] = t[i][1:-1]; i += 1; ex(';')
        while t[i] == 'vertex':
            j = t.index(';', i); p['streams'].append(' '.join(t[i:j]).replace(' . ', '.').replace(' [ ', '[').replace(' ]', ']')); i = j + 1
        for kind in ('vertexShader', 'pixelShader'):
            ex(kind); ver = t[i]; i += 1; name = t[i][1:-1]; i += 1; ex('{'); args = []
            while t[i] != '}':
                j = t.index(';', i); args.append(''.join(t[i:j]).replace('=', ' = ')); i = j + 1
            i += 1
            p['vs' if kind[0] == 'v' else 'ps'] = (ver, name); p['vsargs' if kind[0] == 'v' else 'psargs'] = args
        ex('}'); passes.append(p)
    return passes
def statemap(path):
    t = toks(open(path).read()); i = 0; out = {}
    while i < len(t):
        name = t[i]; i += 1; assert t[i] == '{'; i += 1; rules = []; conds = []
        while t[i] != '}':
            j = i
            while t[j] != ':' : j += 1
            conds.append(' '.join(t[i:j])); i = j + 1
            if t[i] in ('{',) : raise ValueError
            # a rule body follows unless the next tokens are another condition ending in ':'
            k = i
            while t[k] not in (';', ':'): k += 1
            if t[k] == ':': continue
            rules.append((conds, ' '.join(t[i:k]))); conds = []; i = k + 1
        i += 1; out[name] = rules
    return out
R = dict(techsets={}, techs={}, statemaps={})
for f in sorted(os.listdir(os.path.join(root, 'techniquesets'))): R['techsets'][f[:-8]] = techset(os.path.join(root, 'techniquesets', f))
for f in sorted(os.listdir(os.path.join(root, 'techniques'))): R['techs'][f[:-5]] = tech(os.path.join(root, 'techniques', f))
for f in sorted(os.listdir(os.path.join(root, 'statemaps'))): R['statemaps'][f[:-3]] = statemap(os.path.join(root, 'statemaps', f))
json.dump(R, open('tech.json', 'w'), indent=0)
print(len(R['techsets']), 'techsets', len(R['techs']), 'techs', len(R['statemaps']), 'statemaps')
c = C.Counter(s for p in R['techs'].values() for q in p for s in q['streams']); print('streams'); [print('  ', k, v) for k, v in sorted(c.items())]
c = C.Counter(a for p in R['techs'].values() for q in p for a in q['vsargs'] + q['psargs']); print('args'); [print('  ', k, v) for k, v in sorted(c.items())]
c = C.Counter(q['vs'][0] + '/' + q['ps'][0] for p in R['techs'].values() for q in p); print('versions', c)
c = C.Counter(len(p) for p in R['techs'].values()); print('passes per tech', c)
c = C.Counter(q['stateMap'] for p in R['techs'].values() for q in p); print('statemaps used', c)
kw = C.Counter(); vals = C.defaultdict(set)
for sm in R['statemaps'].values():
    for k, rules in sm.items():
        kw[k] += 1
        for conds, body in rules:
            vals[k].add(body)
            for c_ in conds:
                for term in c_.split(' && '): vals['cond'].add(term)
print('statemap keys', kw)
for k, v in vals.items(): print(k, sorted(v))
