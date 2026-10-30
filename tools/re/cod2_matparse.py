"""Parse CoD2 binary materials (materials/*) to exact consumption.
usage: python3 -I matparse.py <materials dir>  -> prints census, writes mats.json"""
import struct, sys, os, json, collections
root = sys.argv[1]
SKIP = {'techniques', 'techniquesets', 'statemaps', 'shaders'}
def cstr(d, o):
    e = d.index(b'\0', o); return d[o:e].decode('latin1'), e + 1
out = []; bad = []
for dp, dn, fn in os.walk(root):
    rel = os.path.relpath(dp, root)
    if rel.split(os.sep)[0] in SKIP: continue
    for f in fn:
        p = os.path.join(dp, f); d = open(p, 'rb').read()
        name_o, ref_o, hashi, sorti, gameFlags, sortKey, atR, atC, maxDeform, deformFlags, usage, toolFlags, locale, atsW, atsH, tess, sflags, contents = struct.unpack_from('<IIHHBBBBfBBHIHHfII', d, 0)
        sb0, sb1, ntex, ncon, ts_o, tex_o, con_o = struct.unpack_from('<IIHHIII', d, 0x2c)
        cover = bytearray(len(d))  # byte ownership
        def own(a, n):
            for i in range(a, a + n):
                if cover[i]: raise ValueError('overlap %x' % i)
                cover[i] = 1
        own(0, 0x44)
        strs = {}
        def s(o):
            v, e = cstr(d, o)
            if o not in strs: strs[o] = v; own(o, e - o)
            return v
        m = dict(file=os.path.relpath(p, root), name=s(name_o), ref=s(ref_o), hashIndex=hashi, sortedIndex=sorti,
                 gameFlags=gameFlags, sortKey=sortKey, atlas=(atR, atC), maxDeformMove=maxDeform, deformFlags=deformFlags,
                 usage=usage, toolFlags=toolFlags, locale=locale, autoTexScale=(atsW, atsH), tessSize=tess,
                 surfaceFlags=sflags, contents=contents, stateBits=(sb0, sb1), techset=s(ts_o), textures=[], constants=[])
        assert tex_o == 0x44, tex_o
        for i in range(ntex):
            o = tex_o + 12 * i; own(o, 12)
            no, samp, sem, u0, u1, uo = struct.unpack_from('<IBBBBI', d, o)
            t = dict(name=s(no), sampler=samp, semantic=sem, unused=(u0, u1))
            if sem == 5:
                own(uo, 32)
                w = struct.unpack_from('<iffffff', d, uo)
                img = struct.unpack_from('<I', d, uo + 28)[0]
                t['water'] = w; t['image'] = s(img) if img else None
            else:
                t['image'] = s(uo)
            m['textures'].append(t)
        for i in range(ncon):
            o = con_o + 20 * i; own(o, 20)
            no, = struct.unpack_from('<I', d, o)
            m['constants'].append(dict(name=s(no), v=struct.unpack_from('<4f', d, o + 4)))
        if not all(cover): bad.append((p, [i for i, c in enumerate(cover) if not c][:8]))
        out.append(m)
print('materials', len(out), 'incomplete', len(bad))
for b in bad[:10]: print(' ', b)
json.dump(out, open('mats.json', 'w'))
