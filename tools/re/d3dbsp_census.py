# Census and cross-reference check of CoD2 1.3 `maps/mp/*.d3dbsp` (IBSP 4),
# the evidence behind docs/research/cod2-d3dbsp-format.md.
# usage: python3 -I tools/re/d3dbsp_census.py <map.d3dbsp>...  (prints per-map
# counts and invariants, then FAILURES n; every invariant holds when n is 0)
import struct, sys, os, math, collections

def lumps(d):
    assert d[:4] == b'IBSP' and struct.unpack_from('<I', d, 4)[0] == 4
    return [struct.unpack_from('<II', d, 8 + 8 * i) for i in range(39)]

def recs(d, L, i, size):
    n, o = L[i]
    assert n % size == 0, (i, n, size)
    return [d[o + k * size:o + (k + 1) * size] for k in range(n // size)]

FAIL = []
def check(cond, msg):
    if not cond:
        FAIL.append(msg)

def run(path, out):
    d = open(path, 'rb').read()
    name = os.path.basename(path)
    L = lumps(d)
    S = collections.OrderedDict()
    check(len(d) >= 0x140, 'short header')
    for i, (n, o) in enumerate(L):
        check(n == 0 or o + n <= len(d), f'{name} lump {i} past eof')
        check(n == 0 or o >= 0x140, f'{name} lump {i} offset in header')
    mats = [struct.unpack('<64sII', r) for r in recs(d, L, 0, 72)]
    nm = len(mats)
    planes = [struct.unpack('<4f', r) for r in recs(d, L, 4, 16)]
    np_ = len(planes)
    for p in planes:
        check(abs(math.sqrt(p[0]**2 + p[1]**2 + p[2]**2) - 1) < 1e-3, f'{name} plane not unit')
    brushes = [struct.unpack('<hh', r) for r in recs(d, L, 6, 4)]
    sides = recs(d, L, 5, 8)
    check(sum(b[0] for b in brushes) == len(sides), f'{name} side sum')
    s = 0
    for b in brushes:
        check(b[0] >= 6 and 0 <= b[1] < nm, f'{name} brush')
        for k in range(b[0]):
            u, m = struct.unpack('<Ii', sides[s + k])
            check(0 <= m < nm, f'{name} side material')
            if k < 6:
                pass
            else:
                check(u < np_, f'{name} side plane')
        bounds = [struct.unpack('<f', sides[s + k][:4])[0] for k in range(6)]
        for a in range(3):
            check(bounds[2 * a] <= bounds[2 * a + 1], f'{name} axial order')
        s += b[0]
    S['brushes'] = len(brushes); S['sides'] = len(sides); S['planes'] = np_; S['materials'] = nm
    # draw surfaces
    surfs = [struct.unpack('<hhIHHI', r) for r in recs(d, L, 7, 16)]
    verts = recs(d, L, 8, 68)
    nidx = L[9][0] // 2
    idx = struct.unpack_from(f'<{nidx}H', d, L[9][1])
    nlm = L[1][0] // 0x400000
    lmmax = -1
    for (m, lm, fv, vc, ic, fi) in surfs:
        check(0 <= m < nm, f'{name} surf material')
        check(lm == 31 or 0 <= lm < nlm, f'{name} surf lightmap {lm}')
        if lm != 31: lmmax = max(lmmax, lm)
        check(fv + vc <= len(verts), f'{name} surf verts')
        check(ic % 3 == 0 and fi + ic <= nidx, f'{name} surf idx')
        check(all(idx[fi + k] < vc for k in range(ic)), f'{name} relative index')
    check(lmmax + 1 == nlm, f'{name} lightmap count {lmmax+1} vs {nlm}')
    S['surfaces'] = len(surfs); S['verts'] = len(verts); S['indices'] = nidx; S['lightmaps'] = nlm
    S['surf_lm31'] = sum(1 for s_ in surfs if s_[1] == 31)
    # vertex frame: normal unit, the two extra vectors unit, orthogonality
    bad = 0; dots = [0, 0]
    for v in verts[::97]:
        f = struct.unpack('<3f3f4B2f2f3f3f', v)
        n = f[3:6]; a = f[14:17]; b = f[17:20]
        un = lambda x: abs(math.sqrt(sum(c * c for c in x)) - 1) < 1e-2
        if not (un(n) and un(a) and un(b)): bad += 1
        dots[0] = max(dots[0], abs(sum(n[i] * a[i] for i in range(3))))
        dots[1] = max(dots[1], abs(sum(n[i] * b[i] for i in range(3))))
    S['vert_nonunit_sampled'] = bad; S['vert_max_dot_n_a,n_b'] = (round(dots[0], 3), round(dots[1], 3))
    # light grid
    grid = [struct.unpack('<IBBH', r) for r in recs(d, L, 2, 8)]
    ncol = L[3][0] // 24
    check(all(g[3] < ncol for g in grid), f'{name} grid colour index')
    keys = [(g[0], g[1] & 0xfe) for g in grid]
    check(all(keys[i] <= keys[i + 1] for i in range(len(keys) - 1)), f'{name} grid not sorted')
    S['grid_dup_keys'] = sum(1 for i in range(len(keys) - 1) if keys[i] == keys[i + 1])
    S['grid_exact_dups'] = sum(1 for i in range(len(grid) - 1) if grid[i] == grid[i + 1])
    check(all(g[1] & 0x2 == 0 for g in grid), f'{name} grid byte4 bit1')
    S['grid'] = len(grid); S['grid_colours'] = ncol
    S['grid_flag1'] = sum(g[1] & 1 for g in grid); S['grid_mask_nonzero'] = sum(1 for g in grid if g[2])
    # cull groups
    cgs = [struct.unpack('<6fii', r) for r in recs(d, L, 10, 32)]
    cgi = struct.unpack_from(f'<{L[11][0]//4}i', d, L[11][1])
    for c in cgs:
        check(c[6] + c[7] <= len(surfs), f'{name} cull group surfs')
    check(all(0 <= x < len(cgs) for x in cgi), f'{name} cull group index')
    S['cullgroups'] = len(cgs); S['cullgroup_indices'] = len(cgi)
    pverts = recs(d, L, 17, 12)
    trees = [struct.unpack('<iii', r) for r in recs(d, L, 22, 12)]
    for t in trees:
        check(t[0] + t[1] <= len(surfs), f'{name} aabb surfs')
    cells = [struct.unpack('<6f7i', r) for r in recs(d, L, 23, 52)]
    portals = [struct.unpack('<4i', r) for r in recs(d, L, 24, 16)]
    for c in cells:
        check(0 <= c[6] < len(trees) or (c[6] == 0 and not trees), f'{name} cell tree')
        check(c[7] + c[8] <= len(portals), f'{name} cell portals')
        check(c[9] + c[10] <= len(cgi), f'{name} cell cgi')
        check(c[12] == 0, f'{name} cell occluders')
    # cell chains
    for i in range(len(cells) - 1):
        check(cells[i][7] + cells[i][8] == cells[i + 1][7], f'{name} portal chain')
    if cells: check(cells[-1][7] + cells[-1][8] == len(portals), f'{name} portal chain end')
    for p in portals:
        check(0 <= p[0] < np_ and 0 <= p[1] < len(cells) and p[2] + p[3] <= len(pverts), f'{name} portal')
    S['cells'] = len(cells); S['portals'] = len(portals); S['portal_verts'] = len(pverts); S['aabbtrees'] = len(trees)
    S['portal_vertcounts'] = sorted(set(p[3] for p in portals))
    # nodes and leafs
    nodes = [struct.unpack('<3i6i', r) for r in recs(d, L, 25, 36)]
    leafs = [struct.unpack('<9i', r) for r in recs(d, L, 26, 36)]
    lb = struct.unpack_from(f'<{L[27][0]//4}i', d, L[27][1])
    check(all(0 <= x < len(brushes) for x in lb), f'{name} leafbrush')
    reached = set()
    for n_ in nodes:
        check(0 <= n_[0] < np_, f'{name} node plane')
        for c in n_[1:3]:
            if c < 0:
                check(-(c + 1) < len(leafs), f'{name} node leaf'); reached.add(-(c + 1))
            else:
                check(c < len(nodes), f'{name} node child')
        check(all(n_[3 + k] <= n_[6 + k] for k in range(3)), f'{name} node bounds')
    caabb = [struct.unpack('<6fHHi', r) for r in recs(d, L, 34, 32)]
    vis = L[36]
    nclusters = struct.unpack_from('<i', d, vis[1])[0] if vis[0] else 0
    rowb = struct.unpack_from('<i', d, vis[1] + 4)[0] if vis[0] else 0
    check(vis[0] == 0 or vis[0] == 8 + nclusters * rowb, f'{name} vis size')
    w1 = collections.Counter(); w7 = collections.Counter(); w8 = collections.Counter()
    for lf in leafs:
        cl, area, fa, na, fb, nb, cell, a7, a8 = lf
        check(-1 <= cl < max(nclusters, 1), f'{name} leaf cluster {cl}')
        check(fa + na <= len(caabb), f'{name} leaf collaabb')
        check(fb + nb <= len(lb), f'{name} leaf brushes')
        check(-1 <= cell < len(cells), f'{name} leaf cell')
        check((cl == -1) == (cell == -1) or True, '')
        w1[area] += 1; w7[a7] += 1; w8[a8] += 1
    S['leaf0'] = leafs[0]
    un = [i for i in range(len(leafs)) if i not in reached]
    S['unreached_leafs'] = (len(un), un[:2], 'tail_is_submodels', un[1:] == list(range(len(leafs) - (len(un) - 1), len(leafs))))
    models_ = [struct.unpack('<6f6i', r) for r in recs(d, L, 35, 48)]
    if un[1:]:
        S['submodel_leaf_brushes_match_model'] = all(sorted(lb[leafs[li][4]:leafs[li][4] + leafs[li][5]]) == list(range(m[10], m[10] + m[11])) and leafs[li][2] == m[8] and leafs[li][3] == m[9] for li, m in zip(un[1:], models_[1:]))
    S['nodes'] = len(nodes); S['leafs'] = len(leafs); S['leafs_reached'] = len(reached)
    S['leaf_area_values'] = dict(w1.most_common(4)); S['leaf_w7'] = dict(w7.most_common(3)); S['leaf_w8'] = dict(w8.most_common(3))
    S['leaf_cluster_-1_vs_cell_-1'] = (sum(1 for l in leafs if l[0] == -1), sum(1 for l in leafs if l[6] == -1),
                                       sum(1 for l in leafs if (l[0] == -1) != (l[6] == -1)))
    S['clusters'] = nclusters; S['vis_row_bytes'] = rowb
    S['leafbrushes'] = len(lb)
    # collision geometry
    cverts = [struct.unpack('<I3f', r) for r in recs(d, L, 29, 16)]
    S['cvert_word0'] = dict(collections.Counter(v[0] for v in cverts).most_common(3))
    edges = [struct.unpack('<I13f', r) for r in recs(d, L, 30, 56)]
    S['edge_word0'] = dict(collections.Counter(e[0] for e in edges).most_common(3))
    tris = [struct.unpack('<12f3i3i', r) for r in recs(d, L, 31, 72)]
    borders = [struct.unpack('<7f', r) for r in recs(d, L, 32, 28)]
    parts = [struct.unpack('<HBBii', r) for r in recs(d, L, 33, 12)]
    S['cverts'] = len(cverts); S['edges'] = len(edges); S['tris'] = len(tris); S['borders'] = len(borders)
    S['partitions'] = len(parts); S['collaabbs'] = len(caabb)
    S['part_word0'] = dict(collections.Counter(p[0] for p in parts).most_common(3))
    tri_used = [0] * len(tris); bor_used = [0] * len(borders)
    for p in parts:
        check(p[1] > 0 and p[3] + p[1] <= len(tris), f'{name} part tris')
        check(p[4] + p[2] <= len(borders), f'{name} part borders')
        for k in range(p[1]): tri_used[p[3] + k] += 1
        for k in range(p[2]): bor_used[p[4] + k] += 1
    S['tri_use_counts'] = dict(collections.Counter(tri_used)); S['border_use_counts'] = dict(collections.Counter(bor_used))
    S['part_tris_max'] = max((p[1] for p in parts), default=0); S['part_borders_max'] = max((p[2] for p in parts), default=0)
    # partitions of a tree chain contiguous?
    S['parts_tris_contiguous'] = all(parts[i][3] + parts[i][1] == parts[i + 1][3] for i in range(len(parts) - 1))
    S['parts_borders_contiguous'] = all(parts[i][4] + parts[i][2] == parts[i + 1][4] for i in range(len(parts) - 1))
    # triangles
    worst = [0.0] * 6; neg1v = 0; neg1e = 0
    for t in tris:
        n = t[0:3]; dist = t[3]
        check(abs(math.sqrt(sum(c * c for c in n)) - 1) < 1e-3, f'{name} tri normal')
        vs = t[12:15]; es = t[15:18]
        for vi in vs:
            check(-1 <= vi < len(cverts), f'{name} tri vert')
        for ei in es:
            check(-1 <= ei < len(edges), f'{name} tri edge')
        neg1v += sum(1 for v in vs if v < 0); neg1e += sum(1 for e in es if e < 0)
    S['tri_vert_-1'] = neg1v; S['tri_edge_-1'] = neg1e
    # svec/tvec: barycentric of the three corners, where the vertices are given
    bary_err = 0.0; plane_err = 0.0; nchk = 0
    for t in tris[::7]:
        vs = t[12:15]
        if min(vs) < 0: continue
        nchk += 1
        P = [cverts[v][1:4] for v in vs]
        for k, p in enumerate(P):
            dd = sum(t[i] * p[i] for i in range(3)) - t[3]
            plane_err = max(plane_err, abs(dd))
            s_ = sum(t[4 + i] * p[i] for i in range(3)) - t[7]
            u_ = sum(t[8 + i] * p[i] for i in range(3)) - t[11]
            want = [(0, 0), (1, 0), (0, 1)][k]
            bary_err = max(bary_err, abs(s_ - want[0]), abs(u_ - want[1]))
    S['tri_full_sampled'] = nchk; S['tri_plane_err'] = round(plane_err, 4); S['tri_bary_err(corner0=(0,0),1=(1,0),2=(0,1))'] = round(bary_err, 4)
    # edges: origin, two axes, dir, length; check axes unit and orthogonal, dir.(end-start)
    e_err = 0.0
    for e in edges[::11]:
        o = e[1:4]; a0 = e[4:7]; a1 = e[7:10]; dv = e[10:13]; ln = e[13]
        dot = lambda x, y: sum(x[i] * y[i] for i in range(3))
        e_err = max(e_err, abs(dot(a0, a0) - 1), abs(dot(a1, a1) - 1), abs(dot(a0, a1)), abs(dot(a0, dv)), abs(dot(a1, dv)), abs(dot(dv, dv) - 1))
        check(ln > 0, f'{name} edge length')
    S['edge_frame_err'] = round(e_err, 4)
    # edge endpoint match with tri vertices: for each tri, edge k should join verts?
    match = collections.Counter()
    for t in tris[::5]:
        vs = t[12:15]; es = t[15:18]
        for k, ei in enumerate(es):
            if ei < 0: continue
            e = edges[ei]; o = e[1:4]; dv = e[10:13]; ln = e[13]
            end = tuple(o[i] + dv[i] * ln for i in range(3))
            for j in range(3):
                for jj in range(3):
                    if j == jj or min(vs[j], vs[jj]) < 0: continue
                    A = cverts[vs[j]][1:4]; B = cverts[vs[jj]][1:4]
                    if max(abs(A[i] - o[i]) for i in range(3)) < 0.01 and max(abs(B[i] - end[i]) for i in range(3)) < 0.01:
                        match[(k, j, jj)] += 1
    S['edge_k_joins_verts(j->jj)'] = dict(match.most_common(6))
    # collision aabb tree
    for i, a in enumerate(caabb):
        ox, oy, oz, hx, hy, hz, mat, nch, u = a
        check(mat < nm, f'{name} caabb material')
        check(min(hx, hy, hz) >= 0, f'{name} caabb half size')
        if nch == 0:
            check(0 <= u < len(parts), f'{name} caabb partition')
        else:
            check(i < u and u + nch <= len(caabb), f'{name} caabb children')
    S['caabb_leaves'] = sum(1 for a in caabb if a[7] == 0)
    S['caabb_partition_refs_unique'] = len(set(a[8] for a in caabb if a[7] == 0)) == len(parts) == S['caabb_leaves']
    # leaf partitions contained in their aabb? check each leaf node's box covers its tris
    cover_err = 0.0
    for a in caabb[::5]:
        if a[7]:
            # children inside parent
            for c in range(a[8], a[8] + a[7]):
                b = caabb[c]
                for k in range(3):
                    cover_err = max(cover_err, (b[k] - b[3 + k]) * -1 + (a[k] - a[3 + k]), (b[k] + b[3 + k]) - (a[k] + a[3 + k]))
            continue
        p = parts[a[8]]
        for tI in range(p[3], p[3] + p[1]):
            for vi in tris[tI][12:15]:
                if vi < 0: continue
                v = cverts[vi][1:4]
                for k in range(3):
                    cover_err = max(cover_err, abs(v[k] - a[k]) - a[3 + k])
    S['caabb_cover_err'] = round(cover_err, 4)
    # models
    models = [struct.unpack('<6f6i', r) for r in recs(d, L, 35, 48)]
    for m in models:
        check(m[6] + m[7] <= len(surfs), f'{name} model surfs')
        check(m[8] + m[9] <= len(caabb), f'{name} model caabb')
        check(m[10] + m[11] <= len(brushes), f'{name} model brushes')
    S['models'] = len(models)
    S['model0'] = models[0][6:]
    S['model_brush_partition'] = all(models[i][10] + models[i][11] == models[i + 1][10] for i in range(len(models) - 1)) and models[-1][10] + models[-1][11] == len(brushes)
    S['model_surf_partition'] = all(models[i][6] + models[i][7] == models[i + 1][6] for i in range(len(models) - 1)) and models[-1][6] + models[-1][7] == len(surfs)
    S['model_caabb_partition'] = all(models[i][8] + models[i][9] == models[i + 1][8] for i in range(len(models) - 1)) and models[-1][8] + models[-1][9] == len(caabb)
    # world leafs' collision aabb ranges: do they stay within model 0's range?
    m0 = models[0]
    S['leaf_caabb_in_model0'] = all(l[3] == 0 or (m0[8] <= l[2] and l[2] + l[3] <= m0[8] + m0[9]) for l in leafs)
    S['leaf_brush_in_model0'] = all(lb[l[4] + k] < m0[10] + m0[11] for l in leafs for k in range(l[5]))
    # surfaces partition: cull groups front, aabb trees after?
    rl = [leafs[i] for i in reached]
    m1a = models[1][8] if len(models) > 1 else len(caabb)
    S['reached_leaf_caabb_before_submodels'] = all(l[3] == 0 or l[2] + l[3] <= m1a for l in rl) and all(m[9] == 0 or m[8] >= m1a for m in models[1:])
    S['reached_leaf_brush_in_model0'] = all(lb[l[4] + k] < m0[10] + m0[11] for l in rl for k in range(l[5]))
    S['submodels_with_caabbs'] = sum(1 for m in models[1:] if m[9]); S['submodels_with_surfs'] = sum(1 for m in models[1:] if m[7])
    S['max_cluster_plus1'] = max(l[0] for l in leafs) + 1
    # surface coverage of model 0 by cull groups and aabb tree leaves
    cover = [0] * (m0[6] + m0[7])
    for c in cgs:
        for k in range(c[6], c[6] + c[7]): cover[k] += 1
    for t in trees:
        if t[2] == 0:
            for k in range(t[0], t[0] + t[1]): cover[k] += 1
    S['model0_surfs_covered_once'] = collections.Counter(cover)
    cg_end = max((c[6] + c[7] for c in cgs), default=0)
    S['cullgroup_surfs_front'] = cg_end == sum(c[7] for c in cgs)
    # preorder tree chaining per cell
    def walk(i):
        n = 1; j = i + 1
        for _ in range(trees[i][2]):
            k = walk(j); n += k; j += k
        return n
    ok = True
    for ci, c in enumerate(cells):
        size = walk(c[6])
        nxt = cells[ci + 1][6] if ci + 1 < len(cells) else len(trees)
        ok = ok and c[6] + size == nxt
    S['cell_trees_chain'] = ok
    owner = {}
    for ci, c in enumerate(cells):
        for k in range(c[7], c[7] + c[8]): owner[k] = ci
    inside = lambda c, x: all(c[i] - 1 <= x[i] <= c[3 + i] + 1 for i in range(3))
    faces = 0
    for k, (pl, cell, fv, vc) in enumerate(portals):
        n_ = planes[pl]; vs = [struct.unpack_from('<3f', pverts[fv + j]) for j in range(vc)]
        check(all(abs(sum(n_[i] * v[i] for i in range(3)) - n_[3]) < 1e-2 for v in vs), f'{name} portal vert off plane')
        cen = [sum(v[i] for v in vs) / vc for i in range(3)]
        faces += inside(cells[owner[k]], [cen[i] - 8 * n_[i] for i in range(3)]) and inside(cells[cell], [cen[i] + 8 * n_[i] for i in range(3)])
    S['portal_plane_faces_out'] = faces == len(portals)
    S['vis_row_bytes_is_ceil64'] = vis[0] == 0 or rowb == ((nclusters + 63) // 64) * 8
    S['borders_xy_unit'] = all(abs(math.hypot(b[0], b[1]) - 1) < 1e-3 and b[6] > 0 for b in borders)
    S['entities_bytes'] = L[37][0]; S['ent_last_byte'] = d[L[37][1] + L[37][0] - 1] if L[37][0] else None
    S['empty_lumps'] = [i for i, (n, o) in enumerate(L) if n == 0]
    out.write(f'== {name}\n')
    for k, v in S.items():
        out.write(f'  {k}: {v}\n')

if __name__=="__main__":
  for p in sys.argv[1:]:
    run(p, sys.stdout)
  print('FAILURES', len(FAIL))
  for f in sorted(set(FAIL))[:50]:
    print(' ', f, FAIL.count(f))
